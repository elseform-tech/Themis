//! The built-in browser: a native Chrome DevTools Protocol client driving a
//! headless Chromium that the agent and the person share.
//!
//! - [`cdp`]: the protocol connection.
//! - [`launch`]: finding and starting a browser with its own profile.
//! - [`snapshot`]: the accessibility outline with refs the model acts on.
//! - [`session`]: per-chat tabs, actions and the Workspace stream (the person and the agent use it at once).
//! - [`tools`]: the agent's `browser_*` tools.

pub mod cdp;
pub mod launch;
pub mod media;
pub mod session;
pub mod snapshot;
pub mod tools;

pub use session::{
    stream_stats, BrowserConfig, BrowserHub, BrowserState, BrowserUpdate, ChatBrowser, StreamStats,
    UserInput,
};
