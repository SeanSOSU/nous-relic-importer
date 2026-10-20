//! What the StarRailSim Sync app and its capture-file decoder share: the inventory exporter, vendored from
//! Reliquary Archiver, the reader that cuts game frames out of a byte stream, and which packets are the game's.

// Vendored as-is (see src/export/VENDORED); not every part of it is used here.
#[allow(dead_code, unused_imports, unused_variables)]
pub mod export;
pub mod frames;
pub mod packets;
