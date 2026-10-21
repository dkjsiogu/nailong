use crate::error::{Error, Result};
use std::thread::JoinHandle;

pub mod catalog;
pub mod fabric;
pub mod forge;
pub mod health;
pub mod modpack;
pub mod neoforge;
pub mod processors;
pub mod slim;
pub mod vanilla;

/// Join a background install step without leaking panic payloads into the UI.
pub fn join_install<T>(handle: JoinHandle<Result<T>>, what: &str) -> Result<T> {
    handle
        .join()
        .map_err(|_| Error::Install(format!("{what}线程异常退出")))?
}
