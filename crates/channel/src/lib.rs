//! Channels: talking to agents from outside Chummy (chummyd's terminal,
//! Matrix, Telegram, Discord), the same in the app and chummyd.
//!
//! - [`Conversations`]: how conversations through any channel behave
//!   (messages, answers, the commands people send ([`Command`]), runs shown
//!   as they go ([`RunView`]), questions, approvals and jobs' reports).
//! - [`ChannelKind`]: a kind of channel, the one place it's described
//!   ([`kind`]): what to ask when one is set up, how it's checked and
//!   described, and how it runs. [`matrix::Matrix`],
//!   [`telegram::Telegram`] and [`discord::Discord`] (running them needs
//!   the `matrix`, `telegram` and `discord` features). The programs never name a kind: they list
//!   [`kinds`] and run one through a [`ChannelHost`] (what differs between
//!   them), a [`Handle`] and a [`ChannelOutlet`]. Each kind verifies people
//!   its own way and asks the program with an [`Event::Verify`], which the
//!   programs show and answer the same way for every kind.
//!
//! A channel only ever does two things for a person: start a run of an
//! agent's own loop with their message, or carry out one of the commands.
//! Commands come only from what people send, never from what an agent says.

mod command;
mod conv;
pub mod discord;
pub mod files;
mod host;
pub mod kind;
pub mod link;
pub mod matrix;
pub mod telegram;
mod view;

pub use command::{Command, HELP, parse};
pub use conv::{
    Approvals, Conv, Conversations, Hub, Left, NOT_HERE, Out, Outlet, channel_chat, is_channel_chat,
};
pub use files::save_to_folder;
pub use host::{
    ChannelHost, Event, Handle, Invite, Person, STALE, Status, Verify, invite, make_private_dir,
    next_number, now_ms,
};
pub use kind::{
    ChannelKind, Field, Input, KindInfo, Options, Secrets, Setup, Start, check_bundle, kind, kinds,
};
pub use link::{ChannelOutlet, FromChannel, ToChannel, parse_verify};
pub use view::{Line, RunView, figures, split, stopwatch};
