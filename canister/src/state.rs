use crate::{
    address_utxoset::AddressUtxoSet,
    block_header_store::BlockHeaderStore,
    metrics::Metrics,
    runtime::{performance_counter, time},
    unstable_blocks::{self, UnstableBlocks},
    validation::ValidationContext,
    UtxoSet,
};
use ic_btc_interface::{Fees, Flag, Height, MillisatoshiPerByte, Network};
use ic_btc_types::{
    into_bitcoin_network, Address, Block, BlockHash, GetSuccessorsCompleteResponse,
    GetSuccessorsPartialResponse, Slicing, OUTPOINT_SIZE,
};
use ic_btc_validation::{validate_header, ValidateHeaderError as InsertBlockError};
use ic_cdk::export::Principal;
use serde::{Deserialize, Serialize};

/// A structure used to maintain the entire state.
// NOTE: `PartialEq` is only available in tests as it would be impractically
// expensive in production.
#[derive(Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
pub struct State {
    /// The UTXOs of all stable blocks since genesis.
    pub utxos: UtxoSet,

    /// Blocks inserted, but are not considered stable yet.
    pub unstable_blocks: UnstableBlocks,

    /// State used for syncing new blocks.
    pub syncing_state: SyncingState,

    /// The canister from which blocks are retrieved.
    /// Defaults to the management canister in production.
    pub blocks_source: Principal,

    /// Cache for the current fee percentiles.
    pub fee_percentiles_cache: Option<FeePercentilesCache>,

    /// A store containing all the stable blocks' headers.
    pub stable_block_headers: BlockHeaderStore,

    /// The fees to charge for each endpoint.
    pub fees: Fees,

    /// Metrics for the various endpoints.
    pub metrics: Metrics,

    /// Flag to control access to the apis provided by the canister.
    pub api_access: Flag,

    /// Flag to determine if the API should be automatically disabled
    /// if the canister isn't fully synced.
    // TODO(EXC-1379): Remove this code once it's deployed to production.
    #[serde(default)]
    pub disable_api_if_not_fully_synced: Flag,
}

impl State {
    /// Create a new blockchain.
    ///
    /// The `stability_threshold` parameter specifies how many confirmations a
    /// block needs before it is considered stable. Stable blocks are assumed
    /// to be final and are never removed.
    pub fn new(stability_threshold: u32, network: Network, genesis_block: Block) -> Self {
        let utxos = UtxoSet::new(network);
        let unstable_blocks =
            UnstableBlocks::new(&utxos, stability_threshold, genesis_block, network);

        Self {
            utxos,
            unstable_blocks,
            syncing_state: SyncingState::default(),
            blocks_source: Principal::management_canister(),
            fee_percentiles_cache: None,
            stable_block_headers: BlockHeaderStore::init(),
            fees: Fees::default(),
            metrics: Metrics::default(),
            api_access: Flag::Enabled,
            disable_api_if_not_fully_synced: Flag::Enabled,
        }
    }

    pub fn network(&self) -> Network {
        self.utxos.network()
    }

    /// The height of the latest stable block.
    pub fn stable_height(&self) -> Height {
        self.utxos.next_height()
    }

    /// Returns the UTXO set of a given bitcoin address.
    pub fn get_utxos(&self, address: Address) -> AddressUtxoSet<'_> {
        AddressUtxoSet::new(address, &self.utxos, &self.unstable_blocks)
    }
}

/// Inserts a block into the state.
/// Returns an error if the block doesn't extend any known block in the state.
pub fn insert_block(state: &mut State, block: Block) -> Result<(), InsertBlockError> {
    let start = performance_counter();
    validate_header(
        &into_bitcoin_network(state.network()),
        &ValidationContext::new(state, block.header())
            .map_err(|_| InsertBlockError::PrevHeaderNotFound)?,
        block.header(),
        time(),
    )?;

    unstable_blocks::push(&mut state.unstable_blocks, &state.utxos, block)
        .expect("Inserting a block with a validated header must succeed.");

    let instructions_count = performance_counter() - start;
    state.metrics.block_insertion.observe(instructions_count);
    Ok(())
}

/// Pops any blocks in `UnstableBlocks` that are considered stable and ingests them to the UTXO set.
///
/// NOTE: This method does a form of time-slicing to stay within the instruction limit, and
/// multiple calls may be required for all the stable blocks to be ingested.
///
/// Returns a bool indicating whether or not the state has changed.
pub fn ingest_stable_blocks_into_utxoset(state: &mut State) -> bool {
    fn pop_block(state: &mut State, ingested_block_hash: BlockHash) {
        let stable_height = state.stable_height();
        // Pop the stable block.
        let popped_block = unstable_blocks::pop(&mut state.unstable_blocks, stable_height);

        // Sanity check that we just popped the same block that was ingested.
        assert_eq!(popped_block.unwrap().block_hash(), ingested_block_hash);
    }

    let prev_state = (
        state.utxos.next_height(),
        &state.utxos.ingesting_block.clone(),
    );
    let has_state_changed = |state: &State| -> bool {
        prev_state != (state.utxos.next_height(), &state.utxos.ingesting_block)
    };

    // Finish ingesting the stable block that's partially ingested, if that exists.
    match state.utxos.ingest_block_continue() {
        None => {}
        Some(Slicing::Paused(())) => return has_state_changed(state),
        Some(Slicing::Done((ingested_block_hash, stats))) => {
            state.metrics.block_ingestion_stats = stats;
            pop_block(state, ingested_block_hash)
        }
    }

    // Check if there are any stable blocks and ingest those into the UTXO set.
    while let Some(new_stable_block) = unstable_blocks::peek(&state.unstable_blocks) {
        // Store the block's header.
        state
            .stable_block_headers
            .insert_block(new_stable_block, state.utxos.next_height());

        match state.utxos.ingest_block(new_stable_block.clone()) {
            Slicing::Paused(()) => return has_state_changed(state),
            Slicing::Done((ingested_block_hash, stats)) => {
                state.metrics.block_ingestion_stats = stats;
                pop_block(state, ingested_block_hash)
            }
        }
    }

    has_state_changed(state)
}

pub fn main_chain_height(state: &State) -> Height {
    unstable_blocks::get_main_chain(&state.unstable_blocks).len() as u32 + state.utxos.next_height()
        - 1
}

pub fn get_unstable_blocks(state: &State) -> Vec<&Block> {
    unstable_blocks::get_blocks(&state.unstable_blocks)
}

// The maximum size in bytes of a bitcoin script for it to be considered "small".
const TX_OUT_SCRIPT_MAX_SIZE_SMALL: u32 = 25;

// The maximum size in bytes of a bitcoin script for it to be considered "medium".
const TX_OUT_SCRIPT_MAX_SIZE_MEDIUM: u32 = 201;

// A transaction output's value in satoshis is a `u64`, which is 8 bytes.
const TX_OUT_VALUE_SIZE: u32 = 8;

const TX_OUT_MAX_SIZE_SMALL: u32 = TX_OUT_SCRIPT_MAX_SIZE_SMALL + TX_OUT_VALUE_SIZE;

const TX_OUT_MAX_SIZE_MEDIUM: u32 = TX_OUT_SCRIPT_MAX_SIZE_MEDIUM + TX_OUT_VALUE_SIZE;

// The height is a `u32`, which is 4 bytes.
const HEIGHT_SIZE: u32 = 4;

/// The size of a key in the UTXOs map, which is an outpoint.
pub const UTXO_KEY_SIZE: usize = OUTPOINT_SIZE as usize;

/// The max size of a value in the "small UTXOs" map.
pub const UTXO_VALUE_MAX_SIZE_SMALL: usize = (TX_OUT_MAX_SIZE_SMALL + HEIGHT_SIZE) as usize;

/// The max size of a value in the "medium UTXOs" map.
pub const UTXO_VALUE_MAX_SIZE_MEDIUM: usize = (TX_OUT_MAX_SIZE_MEDIUM + HEIGHT_SIZE) as usize;

/// A response awaiting to be processed.
#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
pub enum ResponseToProcess {
    /// A complete response that is ready to be processed.
    Complete(GetSuccessorsCompleteResponse),

    /// A partial response that needs more follow-up requests for it to be complete.
    /// The partial response is stored along with the number of pages of the complete
    /// response that has been processed.
    Partial(GetSuccessorsPartialResponse, u8),
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncingState {
    /// Whether or not new blocks should be fetched from the network.
    pub syncing: Flag,

    /// A flag used to ensure that only one request for fetching blocks is
    /// being sent at a time.
    pub is_fetching_blocks: bool,

    /// A response that needs to be processed.
    pub response_to_process: Option<ResponseToProcess>,

    /// The number of rejects received when calling GetSuccessors.
    pub num_get_successors_rejects: u64,

    /// The number of errors occurred when deserializing blocks.
    pub num_block_deserialize_errors: u64,

    /// The number of errors occurred when inserting a block.
    pub num_insert_block_errors: u64,
}

impl Default for SyncingState {
    fn default() -> Self {
        Self {
            syncing: Flag::Enabled,
            is_fetching_blocks: false,
            response_to_process: None,
            num_get_successors_rejects: 0,
            num_block_deserialize_errors: 0,
            num_insert_block_errors: 0,
        }
    }
}

/// Cache for storing last calculated fee percentiles
///
/// Stores last tip block hash and fee percentiles associated with it.
#[derive(Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeePercentilesCache {
    pub tip_block_hash: BlockHash,
    pub fee_percentiles: Vec<MillisatoshiPerByte>,
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_utils::build_chain;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10))]
        #[test]
        fn serialize_deserialize_state(
            stability_threshold in 1..150u32,
            num_blocks in 1..250u32,
            num_transactions_in_block in 1..100u32,
        ) {
            let network = Network::Regtest;
            let blocks = build_chain(network, num_blocks, num_transactions_in_block);

            let mut state = State::new(stability_threshold, network, blocks[0].clone());

            for block in blocks[1..].iter() {
                insert_block(&mut state, block.clone()).unwrap();
                ingest_stable_blocks_into_utxoset(&mut state);
            }

            let mut bytes = vec![];
            ciborium::ser::into_writer(&state, &mut bytes).unwrap();
            let new_state: State = ciborium::de::from_reader(&bytes[..]).unwrap();

            // Verify the new state is the same as the old state.
            assert!(state == new_state);
        }
    }

    #[test]
    fn block_ingestion_stats_are_updated() {
        let stability_threshold = 0;
        let num_blocks = 3;
        let num_transactions_per_block = 10;
        let network = Network::Regtest;
        let blocks = build_chain(network, num_blocks, num_transactions_per_block);

        let mut state = State::new(stability_threshold, network, blocks[0].clone());

        assert_eq!(state.stable_height(), 0);
        insert_block(&mut state, blocks[1].clone()).unwrap();

        // The genesis block is now stable. Ingest it.
        let metrics_before = state.metrics.block_ingestion_stats.clone();
        ingest_stable_blocks_into_utxoset(&mut state);
        assert_eq!(state.stable_height(), 1);

        // Verify that the stats have been updated.
        assert_ne!(metrics_before, state.metrics.block_ingestion_stats);

        // Ingest the next block. This time, the performance counter is set so that
        // the ingestion is time-sliced.
        crate::runtime::set_performance_counter_step(1_000_000_000);

        insert_block(&mut state, blocks[2].clone()).unwrap();
        let metrics_before = state.metrics.block_ingestion_stats.clone();
        let mut num_rounds = 0;
        while state.stable_height() == 1 {
            assert_eq!(metrics_before, state.metrics.block_ingestion_stats);
            ingest_stable_blocks_into_utxoset(&mut state);
            crate::runtime::performance_counter_reset();
            num_rounds += 1;
        }

        // Assert that the block has been ingested.
        assert_eq!(state.stable_height(), 2);

        // Assert that the block ingestion has been time-sliced.
        assert!(num_rounds > 1);

        // Assert the stats have been updated.
        assert_ne!(metrics_before, state.metrics.block_ingestion_stats);
    }
}
