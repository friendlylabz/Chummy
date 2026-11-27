//! Channels: talking to agents from outside Chummy (chummyd's terminal,
//! Matrix, Telegram), the same in the app and chummyd.
//!
//! - [`Conversations`]: how conversations through any channel behave
//!   (messages, answers, the commands people send ([`Command`]), runs shown
//!   as they go ([`RunView`]), questions, approvals and jobs' reports).
//! - One implementation per kind of channel, which finds conversations and
//!   carries text: [`matrix`] (with the `matrix` feature) and [`telegram`]
//!   (with the `telegram` feature). Each runs behind a [`ChannelHost`] (what
//!   differs between the programs) and is driven through a [`Handle`] and
//!   [`ChannelOutlet`].
//!
//! A channel only ever does two things for a person: start a run of an
//! agent's own loop with their message, or carry out one of the commands.
//! Commands come only from what people send, never from what an agent says.

mod command;
mod conv;
pub mod files;
mod host;
pub mod link;
#[cfg(feature = "matrix")]
pub mod matrix;
#[cfg(feature = "telegram")]
pub mod telegram;
mod view;

pub use command::{Command, HELP, parse};
pub use conv::{
    Approvals, Conv, Conversations, Hub, Left, NOT_HERE, Out, Outlet, channel_chat, is_channel_chat,
};
pub use files::save_to_folder;
pub use host::{
    ChannelHost, Code, Event, Handle, Person, STALE, Status, make_private_dir, next_number, now_ms,
};
pub use link::{ChannelOutlet, FromChannel, ToChannel, parse_verify};
pub use view::{Line, RunView, figures, split, stopwatch};
