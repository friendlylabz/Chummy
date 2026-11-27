//! Channels: talking to agents from outside Chummy (chummyd's terminal,
//! Matrix, and Telegram later), the same in the app and chummyd.
//!
//! - [`Conversations`]: how conversations through any channel behave
//!   (messages, answers, the commands people send ([`Command`]), runs shown
//!   as they go ([`RunView`]), questions, approvals and jobs' reports).
//! - One implementation per kind of channel, which finds conversations and
//!   carries text: [`matrix`] (with the `matrix` feature).
//!
//! A channel only ever does two things for a person: start a run of an
//! agent's own loop with their message, or carry out one of the commands.
//! Commands come only from what people send, never from what an agent says.

mod command;
mod conv;
pub mod link;
#[cfg(feature = "matrix")]
pub mod matrix;
mod view;

pub use command::{Command, HELP, parse};
pub use conv::{
    Approvals, Conv, Conversations, Hub, Left, NOT_HERE, Out, Outlet, channel_chat, is_channel_chat,
};
pub use link::{FromMatrix, MatrixOutlet, ToMatrix, parse_verify};
pub use view::{Line, RunView, figures, split, stopwatch};
