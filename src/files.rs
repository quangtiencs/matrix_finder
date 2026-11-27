//! Files and where they live: local folders, archives and servers, and the operations
//! that change them (copy, move, delete, safe writes).
pub mod archive;
pub mod dirwatch;
pub mod drives;
pub mod gitinfo;
pub mod ops;
pub mod remote;
pub mod safefs;
pub mod tempcopy;
pub mod vfs;
