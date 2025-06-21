use crate::PartManError;

pub trait PartBlock {
    fn read_block(&mut self, block_id: u64, buf: &mut [u8]) -> Result<(), PartManError>;
    fn write_block(&mut self, block_id: u64, buf: &[u8]) -> Result<(), PartManError>;
    fn block_size(&self) -> usize;
    fn num_blocks(&self) -> u64;
}
