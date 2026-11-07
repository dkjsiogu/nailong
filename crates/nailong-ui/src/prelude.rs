//! 每个模块开头写一行 `use crate::prelude::*;`：外部库、核心库、数据类型和通用组件一次带齐，
//! 模块之间不用互相记路径。新的公共组件放进 `widgets`，新的数据类型放进 `model`，这里就会自动带上。

pub(crate) use crate::theme as t;
pub(crate) use crate::{motion, rings};
pub(crate) use eframe::egui;
pub(crate) use nailong_core::auth;
pub(crate) use nailong_core::bench;
pub(crate) use nailong_core::install::{
    catalog, fabric, forge, health, modpack, neoforge, vanilla,
};
pub(crate) use nailong_core::installed;
pub(crate) use nailong_core::launch::{self, LaunchOptions};
pub(crate) use nailong_core::launchcfg::{self, MemoryPick, VersionLaunch};
pub(crate) use nailong_core::learned;
pub(crate) use nailong_core::mods;
pub(crate) use nailong_core::progress::{Progress, Sink, from_fn};
pub(crate) use nailong_core::rollback;
pub(crate) use nailong_core::saves;
pub(crate) use nailong_core::snapshots;
pub(crate) use nailong_core::tuning;
pub(crate) use nailong_core::{Error, Root, cancel, java};
pub(crate) use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
pub(crate) use std::path::PathBuf;
pub(crate) use std::sync::{Arc, Mutex, mpsc};
pub(crate) use std::time::{Duration, Instant};

pub(crate) use crate::activity::*;
pub(crate) use crate::app::*;
pub(crate) use crate::dialogs::account::*;
pub(crate) use crate::dialogs::add_account::*;
pub(crate) use crate::dialogs::log;
pub(crate) use crate::dialogs::rollback::RollbackState;
pub(crate) use crate::dialogs::settings::*;
pub(crate) use crate::dialogs::update::{UpdateState, update_pill};
pub(crate) use crate::fonts::*;
pub(crate) use crate::model::*;
pub(crate) use crate::msg::*;
pub(crate) use crate::pages::content::drop::*;
pub(crate) use crate::pages::content::library::*;
pub(crate) use crate::pages::content::*;
pub(crate) use crate::pages::home::HomeState;
pub(crate) use crate::pages::shots;
pub(crate) use crate::pages::versions::*;
pub(crate) use crate::pages::worlds::WorldsState;
pub(crate) use crate::play::PlayState;
pub(crate) use crate::widgets::*;
