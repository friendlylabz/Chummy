//! Commands: a message starting with `/`.

/// Something a person asks chummyd to do, rather than say to the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Stop the run going on in this conversation.
    Stop,
    /// Pause it (carry on later with `/resume`).
    Pause,
    /// Carry on this conversation's paused run.
    Resume,
    /// Throw away this conversation's paused run.
    Discard,
    /// Start a fresh conversation.
    New,
    /// What's going on: runs, paused runs, questions waiting.
    Status,
    /// The agent's jobs, and when they run next.
    Jobs,
    /// Run one of the agent's jobs now (by id or name).
    Run(String),
    /// Allow the tool call waiting for approval.
    Allow,
    /// Refuse it, with an optional reason.
    Deny(String),
    /// The agents to talk to (terminal only: it has no rooms).
    Agents,
    /// Talk to another agent (terminal only).
    Agent(String),
    /// Stop chummyd (terminal only: whoever has the terminal can anyway).
    Quit,
    Help,
}

/// What each command does, for `/help`.
pub const HELP: &str = "\
/stop            stop the run going on
/pause           pause it; /resume carries on where it stopped
/resume          carry on the paused run
/discard         throw away the paused run
/new             start a fresh conversation
/status          what's going on
/jobs            this agent's jobs
/run <job>       run a job now
/allow           allow the tool call waiting for you
/deny [reason]   refuse it
/help            this list
Anything else is a message to the agent. If it asked you something, your next message is the answer.";

/// Reads a message as a command. `None`: it isn't one (it doesn't start
/// with `/`), so it's for the agent. `Some(Err)`: it looks like a command
/// but isn't one chummyd knows, and is never passed to the agent.
pub fn parse(text: &str) -> Option<Result<Command, String>> {
    let text = text.trim();
    let rest = text.strip_prefix('/')?;
    let (name, arg) = match rest.split_once(char::is_whitespace) {
        Some((n, a)) => (n, a.trim()),
        None => (rest, ""),
    };
    let needs_none = |c: Command| {
        if arg.is_empty() {
            Ok(c)
        } else {
            Err(format!("/{name} takes nothing after it."))
        }
    };
    Some(match name.to_lowercase().as_str() {
        "stop" => needs_none(Command::Stop),
        "pause" => needs_none(Command::Pause),
        "resume" => needs_none(Command::Resume),
        "discard" => needs_none(Command::Discard),
        "new" => needs_none(Command::New),
        "status" => needs_none(Command::Status),
        "jobs" => needs_none(Command::Jobs),
        "help" | "?" => needs_none(Command::Help),
        "allow" => needs_none(Command::Allow),
        "deny" => Ok(Command::Deny(arg.to_string())),
        "agents" => needs_none(Command::Agents),
        "quit" | "exit" => needs_none(Command::Quit),
        "run" if arg.is_empty() => Err("Say which job: /run <job>.".into()),
        "run" => Ok(Command::Run(arg.to_string())),
        "agent" if arg.is_empty() => Err("Say which agent: /agent <name>.".into()),
        "agent" => Ok(Command::Agent(arg.to_string())),
        _ => Err(format!(
            "Unknown command: /{}. Send /help for the list.",
            name.chars().take(40).collect::<String>()
        )),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_read_as_commands_and_everything_else_is_a_message() {
        assert_eq!(parse("hello"), None);
        assert_eq!(parse("  not /stop"), None);
        assert_eq!(parse("/stop"), Some(Ok(Command::Stop)));
        assert_eq!(parse(" /PAUSE "), Some(Ok(Command::Pause)));
        assert_eq!(
            parse("/run Morning digest"),
            Some(Ok(Command::Run("Morning digest".into())))
        );
        assert_eq!(
            parse("/deny not now"),
            Some(Ok(Command::Deny("not now".into())))
        );
        assert_eq!(parse("/deny"), Some(Ok(Command::Deny(String::new()))));
        assert!(matches!(parse("/run"), Some(Err(_))));
        assert!(matches!(parse("/stop now"), Some(Err(_))));
        let e = parse("/rm -rf").unwrap().unwrap_err();
        assert!(e.contains("Unknown command: /rm"), "{e}");
        assert!(matches!(parse("/"), Some(Err(_))));
    }
}
