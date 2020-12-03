mod debug_util;
mod realearn_editor;
use realearn_editor::*;
mod app;
pub use app::*;
mod realearn_plugin;
mod realearn_plugin_parameters;

#[allow(unused)]
mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

vst::plugin_main!(realearn_plugin::RealearnPlugin);
