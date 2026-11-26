//! What every chummyd channel shares, whatever carries the messages
//! (the terminal now, Matrix and Telegram later): the commands people send
//! ([`Command`]) and how a run reads as one message ([`RunView`]).
//!
//! A channel only ever does two things for a person: start a run of an
//! agent's own loop with their message, or carry out one of these commands.
//! Commands come only from what people send, never from what an agent says.

mod command;
mod view;

pub use command::{Command, HELP, parse};
pub use view::{Line, RunView, figures, stopwatch};
