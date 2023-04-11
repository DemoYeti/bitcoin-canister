use crate::genesis_block;
use bitcoin::{
    hashes::Hash, secp256k1::rand::rngs::OsRng, secp256k1::Secp256k1, Address as BitcoinAddress,
    BlockHeader, PublicKey, Script, WScriptHash,
};
use ic_btc_interface::Network;
use ic_btc_test_utils::{
    BlockBuilder as ExternalBlockBuilder, TransactionBuilder as ExternalTransactionBuilder,
};
use ic_btc_types::{into_bitcoin_network, Address, Block, OutPoint, Transaction};
use ic_stable_structures::{BoundedStorable, Memory, StableBTreeMap};
use proptest::prelude::RngCore;
use std::str::FromStr;

/// Generates a random P2PKH address.
pub fn random_p2pkh_address(network: Network) -> Address {
    let secp = Secp256k1::new();
    let mut rng = OsRng::new().unwrap();

    BitcoinAddress::p2pkh(
        &PublicKey::new(secp.generate_keypair(&mut rng).1),
        into_bitcoin_network(network),
    )
    .into()
}

pub fn random_p2tr_address(network: Network) -> Address {
    ic_btc_test_utils::random_p2tr_address(into_bitcoin_network(network)).into()
}

pub fn random_p2wpkh_address(network: Network) -> Address {
    let secp = Secp256k1::new();
    let mut rng = OsRng::new().unwrap();
    BitcoinAddress::p2wpkh(
        &PublicKey::new(secp.generate_keypair(&mut rng).1),
        into_bitcoin_network(network),
    )
    .expect("failed to create p2wpkh address")
    .into()
}

pub fn random_p2wsh_address(network: Network) -> Address {
    let mut rng = OsRng::new().unwrap();
    let mut hash = [0u8; 32];
    rng.fill_bytes(&mut hash);
    BitcoinAddress::p2wsh(
        &Script::new_v0_p2wsh(&WScriptHash::from_hash(Hash::from_slice(&hash).unwrap())),
        into_bitcoin_network(network),
    )
    .into()
}

/// Builds a random chain with the given number of block and transactions.
/// The genesis block used in the chain is also random.
pub fn build_chain(
    network: Network,
    num_blocks: u32,
    num_transactions_per_block: u32,
) -> Vec<Block> {
    build_chain_with_genesis_block(
        network,
        BlockBuilder::genesis().build(),
        num_blocks,
        num_transactions_per_block,
    )
}

/// Builds a random chain with the given number of block and transactions
/// and starting with the Regtest genesis block.
pub fn build_regtest_chain(num_blocks: u32, num_transactions_per_block: u32) -> Vec<Block> {
    let network = Network::Regtest;
    build_chain_with_genesis_block(
        network,
        genesis_block(network),
        num_blocks,
        num_transactions_per_block,
    )
}

fn build_chain_with_genesis_block(
    network: Network,
    genesis_block: Block,
    num_blocks: u32,
    num_transactions_per_block: u32,
) -> Vec<Block> {
    let address = random_p2pkh_address(network);
    let mut blocks = vec![genesis_block.clone()];
    let mut prev_block: Block = genesis_block;
    let mut value = 1;

    // Since we start with a genesis block, we need `num_blocks - 1` additional blocks.
    for _ in 0..num_blocks - 1 {
        let mut block_builder = BlockBuilder::with_prev_header(prev_block.header());
        let mut transactions = vec![];
        for _ in 0..num_transactions_per_block {
            transactions.push(
                TransactionBuilder::coinbase()
                    .with_output(&address, value)
                    .build(),
            );
            // Vary the value of the transaction to ensure that
            // we get unique outpoints in the blockchain.
            value += 1;
        }

        for transaction in transactions.iter() {
            block_builder = block_builder.with_transaction(transaction.clone());
        }

        let block = block_builder.build();
        blocks.push(block.clone());
        prev_block = block;
    }

    blocks
}

/// Returns true if the instances of `StableBTreeMap` provided are equal.
pub fn is_stable_btreemap_equal<
    M: Memory,
    K: BoundedStorable + Ord + Eq + Clone,
    V: BoundedStorable + Eq,
>(
    a: &StableBTreeMap<K, V, M>,
    b: &StableBTreeMap<K, V, M>,
) -> bool {
    if a.len() != b.len() {
        return false;
    }

    for (x, y) in a.iter().zip(b.iter()) {
        if x != y {
            return false;
        }
    }

    true
}

/// A wrapper around `ic_btc_test_utils::BlockBuilder` that returns `ic_btc_types::Block`
/// as opposed to `bitcoin::Block`.
pub struct BlockBuilder {
    builder: ExternalBlockBuilder,
}

impl BlockBuilder {
    pub fn genesis() -> Self {
        Self {
            builder: ExternalBlockBuilder::genesis(),
        }
    }

    pub fn with_prev_header(prev_header: &BlockHeader) -> Self {
        Self {
            builder: ExternalBlockBuilder::with_prev_header(*prev_header),
        }
    }

    pub fn with_transaction(self, transaction: Transaction) -> Self {
        Self {
            builder: self.builder.with_transaction(transaction.into()),
        }
    }

    pub fn build(self) -> Block {
        Block::new(self.builder.build())
    }

    pub fn build_with_mock_difficulty(self, mock_difficulty: u64) -> Block {
        let mut block = self.build();
        block.mock_difficulty = Some(mock_difficulty);
        block
    }
}

/// A wrapper around `ic_btc_test_utils::TransactionBuilder` that returns
/// `ic_btc_types::Transaction` as opposed to `bitcoin::Transaction`.
pub struct TransactionBuilder {
    builder: ExternalTransactionBuilder,
}

impl TransactionBuilder {
    pub fn new() -> Self {
        Self {
            builder: ExternalTransactionBuilder::new(),
        }
    }

    pub fn coinbase() -> Self {
        Self {
            builder: ExternalTransactionBuilder::coinbase(),
        }
    }

    pub fn with_input(self, previous_output: OutPoint) -> Self {
        Self {
            builder: self.builder.with_input(previous_output.into()),
        }
    }

    pub fn with_lock_time(self, i: u32) -> Self {
        Self {
            builder: self.builder.with_lock_time(i),
        }
    }

    pub fn with_output(self, address: &Address, value: u64) -> Self {
        Self {
            builder: self.builder.with_output(
                &BitcoinAddress::from_str(&address.to_string()).unwrap(),
                value,
            ),
        }
    }

    pub fn build(self) -> Transaction {
        Transaction::new(self.builder.build())
    }
}

pub struct BlockChainBuilder {
    num_blocks: u32,
    prev_block_header: Option<BlockHeader>,
}

impl BlockChainBuilder {
    pub fn new(num_blocks: u32) -> Self {
        Self {
            num_blocks,
            prev_block_header: None,
        }
    }

    pub fn fork(prev_block: &Block, num_blocks: u32) -> Self {
        Self {
            num_blocks,
            prev_block_header: Some(*prev_block.header()),
        }
    }

    pub fn build(self) -> Vec<Block> {
        let mut blocks = Vec::with_capacity(self.num_blocks as usize);

        match self.prev_block_header {
            None => {
                blocks.push(genesis_block(Network::Regtest));
            }
            Some(prev_block_header) => {
                let block = BlockBuilder::with_prev_header(&prev_block_header).build();
                blocks.push(block);
            }
        };

        for i in 1..self.num_blocks as usize {
            let block = BlockBuilder::with_prev_header(blocks[i - 1].header()).build();
            blocks.push(block);
        }

        blocks
    }
}
