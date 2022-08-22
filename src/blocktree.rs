use bitcoin::{Block, BlockHash};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Represents a non-empty block chain as:
/// * the first block of the chain
/// * the successors to this block (which can be an empty list)
#[derive(Debug, PartialEq, Eq)]
pub struct BlockChain<'a> {
    // The first block of this `BlockChain`, i.e. the one at the lowest height.
    first: &'a Block,
    // The successor blocks of this `BlockChain`, i.e. the chain after the
    // `first` block.
    successors: Vec<&'a Block>,
}

impl<'a> BlockChain<'a> {
    /// Creates a new `BlockChain` with the given `first` block and an empty list
    /// of successors.
    pub fn new(first: &'a Block) -> Self {
        Self {
            first,
            successors: vec![],
        }
    }

    /// This is only useful for tests to simplify the creation of a `BlockChain`.
    #[cfg(test)]
    pub fn new_with_successors(first: &'a Block, successors: Vec<&'a Block>) -> Self {
        Self { first, successors }
    }

    /// Appends a new block to the list of `successors` of this `BlockChain`.
    pub fn push(&mut self, block: &'a Block) {
        self.successors.push(block);
    }

    /// Returns the length of this `BlockChain`.
    pub fn len(&self) -> usize {
        self.successors.len() + 1
    }

    pub fn first(&self) -> &'a Block {
        self.first
    }

    /*pub fn tip(&self) -> &'a Block {
        match self.successors.last() {
            None => {
                // The chain consists of only one block, and that is the tip.
                self.first
            }
            Some(tip) => tip,
        }
    }*/

    /// Consumes this `BlockChain` and returns the entire chain of blocks.
    pub fn into_chain(self) -> Vec<&'a Block> {
        let mut chain = vec![self.first];
        chain.extend(self.successors);
        chain
    }
}

/// Error returned when attempting to create a `BlockChain` out of an empty
/// list of blocks.
#[derive(Debug)]
pub struct EmptyChainError {}

impl fmt::Display for EmptyChainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot create a `BlockChain` from an empty chain")
    }
}

/// Maintains a tree of connected blocks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Eq)]
pub struct BlockTree {
    #[serde(serialize_with = "serialize_block")]
    #[serde(deserialize_with = "deserialize_block")]
    pub root: Block,
    pub children: Vec<BlockTree>,
}

impl BlockTree {
    /// Creates a new `BlockTree` with the given block as its root.
    pub fn new(root: Block) -> Self {
        Self {
            root,
            children: vec![],
        }
    }
}

/// Extends the tree with the given block.
///
/// Blocks can extend the tree in the following cases:
///   * The block is already present in the tree (no-op).
///   * The block is a successor of a block already in the tree.
pub fn extend(block_tree: &mut BlockTree, block: Block) -> Result<(), BlockDoesNotExtendTree> {
    if contains(block_tree, &block) {
        // The block is already present in the tree. Nothing to do.
        return Ok(());
    }

    // Check if the block is a successor to any of the blocks in the tree.
    match find_mut(block_tree, &block.header.prev_blockhash) {
        Some(block_subtree) => {
            assert_eq!(block_subtree.root.block_hash(), block.header.prev_blockhash);
            // Add the block as a successor.
            block_subtree.children.push(BlockTree::new(block));
            Ok(())
        }
        None => Err(BlockDoesNotExtendTree(block)),
    }
}

/// Returns all the blockchains in the tree.
pub fn blockchains(block_tree: &BlockTree) -> Vec<BlockChain> {
    if block_tree.children.is_empty() {
        return vec![BlockChain {
            first: &block_tree.root,
            successors: vec![],
        }];
    }

    let mut tips = vec![];
    for child in block_tree.children.iter() {
        tips.extend(
            blockchains(child)
                .into_iter()
                .map(|bc| BlockChain {
                    first: &block_tree.root,
                    successors: bc.into_chain(),
                })
                .collect::<Vec<BlockChain>>(),
        );
    }

    tips
}

/// Returns a `BlockChain` starting from the anchor and ending with the `tip`.
///
/// If the `tip` doesn't exist in the tree, `None` is returned.
pub fn get_chain_with_tip<'a, 'b>(
    block_tree: &'a BlockTree,
    tip: &'b BlockHash,
) -> Option<BlockChain<'a>> {
    // Compute the chain in reverse order, as that's more efficient, and then
    // reverse it to get the answer in the correct order.
    get_chain_with_tip_reverse(block_tree, tip).map(|mut chain| {
        // Safe to unwrap as the `chain` would contain at least the root of the
        // `BlockTree` it was produced from.
        // This would be the first block since the chain is in reverse order.
        let first = chain.pop().unwrap();
        // Reverse the chain to get the list of `successors` in the right order.
        chain.reverse();
        BlockChain {
            first,
            successors: chain,
        }
    })
}

// Do a depth-first search to find the blockchain that ends with the given `tip`.
// For performance reasons, the list is returned in the reverse order, starting
// from `tip` and ending with `anchor`.
fn get_chain_with_tip_reverse<'a, 'b>(
    block_tree: &'a BlockTree,
    tip: &'b BlockHash,
) -> Option<Vec<&'a Block>> {
    if block_tree.root.block_hash() == *tip {
        return Some(vec![&block_tree.root]);
    }

    for child in block_tree.children.iter() {
        if let Some(mut chain) = get_chain_with_tip_reverse(child, tip) {
            chain.push(&block_tree.root);
            return Some(chain);
        }
    }

    None
}

/// Returns the depth of the tree.
pub fn depth(block_tree: &BlockTree) -> u32 {
    if block_tree.children.is_empty() {
        return 0;
    }

    let mut max_child_depth = 0;

    for child in block_tree.children.iter() {
        max_child_depth = std::cmp::max(1 + depth(child), max_child_depth);
    }

    max_child_depth
}

// Returns a `BlockTree` where the hash of the root block matches the provided `block_hash`
// if it exists, and `None` otherwise.
fn find_mut<'a>(block_tree: &'a mut BlockTree, blockhash: &BlockHash) -> Option<&'a mut BlockTree> {
    if block_tree.root.block_hash() == *blockhash {
        return Some(block_tree);
    }

    for child in block_tree.children.iter_mut() {
        if let res @ Some(_) = find_mut(child, blockhash) {
            return res;
        }
    }

    None
}

// Returns true if a block exists in the tree, false otherwise.
fn contains(block_tree: &BlockTree, block: &Block) -> bool {
    if block_tree.root.block_hash() == block.block_hash() {
        return true;
    }

    for child in block_tree.children.iter() {
        if contains(child, block) {
            return true;
        }
    }

    false
}

/// An error thrown when trying to add a block that isn't a successor
/// of any block in the tree.
#[derive(Debug)]
pub struct BlockDoesNotExtendTree(pub Block);

// A method for serde to serialize a block.
// Serialization relies on converting the block into a blob using the
// Bitcoin standard format.
fn serialize_block<S>(block: &Block, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    use bitcoin::consensus::Encodable;
    let mut bytes = vec![];
    Block::consensus_encode(block, &mut bytes).unwrap();
    serde_bytes::serialize(&bytes, s)
}

// A method for serde to deserialize a block.
// The blob is assumed to be in Bitcoin standard format.
fn deserialize_block<'de, D>(d: D) -> Result<Block, D::Error>
where
    D: Deserializer<'de>,
{
    let bytes: Vec<u8> = serde_bytes::deserialize(d).unwrap();
    use bitcoin::consensus::Decodable;
    Ok(Block::consensus_decode(bytes.as_slice()).unwrap())
}

#[cfg(test)]
mod test {
    use super::*;
    use ic_btc_test_utils::BlockBuilder;

    #[test]
    fn tree_single_block() {
        let block_tree = BlockTree::new(BlockBuilder::genesis().build());

        assert_eq!(depth(&block_tree), 0);
        assert_eq!(
            blockchains(&block_tree),
            vec![BlockChain {
                first: &block_tree.root,
                successors: vec![],
            }]
        );
    }

    #[test]
    fn tree_multiple_forks() {
        let genesis_block = BlockBuilder::genesis().build();
        let genesis_block_header = genesis_block.header;
        let mut block_tree = BlockTree::new(genesis_block);

        for i in 1..5 {
            // Create different blocks extending the genesis block.
            // Each one of these should be a separate fork.
            extend(
                &mut block_tree,
                BlockBuilder::with_prev_header(genesis_block_header).build(),
            )
            .unwrap();
            assert_eq!(blockchains(&block_tree).len(), i);
        }

        assert_eq!(depth(&block_tree), 1);
    }

    #[test]
    fn chain_with_tip_no_forks() {
        let mut blocks = vec![BlockBuilder::genesis().build()];
        for i in 1..10 {
            blocks.push(BlockBuilder::with_prev_header(blocks[i - 1].header).build())
        }

        let mut block_tree = BlockTree::new(blocks[0].clone());

        for block in blocks.iter() {
            extend(&mut block_tree, block.clone()).unwrap();
        }

        for (i, block) in blocks.iter().enumerate() {
            // Fetch the blockchain with the `block` as tip.
            let chain = get_chain_with_tip(&block_tree, &block.block_hash())
                .unwrap()
                .into_chain();

            // The first block should be the genesis block.
            assert_eq!(chain[0], &blocks[0]);
            // The last block should be the expected tip.
            assert_eq!(chain.last().unwrap(), &block);

            // The length of the chain should grow as the requested tip gets deeper.
            assert_eq!(chain.len(), i + 1);

            // All blocks should be correctly chained to one another.
            for i in 1..chain.len() {
                assert_eq!(chain[i - 1].block_hash(), chain[i].header.prev_blockhash)
            }
        }
    }

    #[test]
    fn chain_with_tip_multiple_forks() {
        let mut blocks = vec![BlockBuilder::genesis().build()];
        let mut block_tree = BlockTree::new(blocks[0].clone());

        let num_forks = 5;
        for _ in 0..num_forks {
            for i in 1..10 {
                blocks.push(BlockBuilder::with_prev_header(blocks[i - 1].header).build())
            }

            for block in blocks.iter() {
                extend(&mut block_tree, block.clone()).unwrap();
            }

            for (i, block) in blocks.iter().enumerate() {
                // Fetch the blockchain with the `block` as tip.
                let chain = get_chain_with_tip(&block_tree, &block.block_hash())
                    .unwrap()
                    .into_chain();

                // The first block should be the genesis block.
                assert_eq!(chain[0], &blocks[0]);
                // The last block should be the expected tip.
                assert_eq!(chain.last().unwrap(), &block);

                // The length of the chain should grow as the requested tip gets deeper.
                assert_eq!(chain.len(), i + 1);

                // All blocks should be correctly chained to one another.
                for i in 1..chain.len() {
                    assert_eq!(chain[i - 1].block_hash(), chain[i].header.prev_blockhash)
                }
            }

            blocks = vec![blocks[0].clone()];
        }
    }
}
