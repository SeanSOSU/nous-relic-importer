//! The live importer the desktop and Android apps share: the game's captured packets in, through Reliquary's
//! decoder, and the account out to every StarRailSim page connected to the local socket. Each app brings only
//! its packet source.

pub mod accounts;
pub mod collector;
pub mod decoder;
pub mod hub;
pub mod server;
