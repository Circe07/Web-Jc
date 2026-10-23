/// Machine state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum State {
    Idle,
    Running,
    Paused,
}

/// Commands a user can send.
#[derive(Debug, Clone, Copy)]
pub enum Command {
    Start,
    Stop,
    Pause,
    Resume,
}

/// Applies a command to a state.
pub fn apply(state: State, cmd: Command) -> State {
    match cmd {
        Command::Start => State::Running,
        Command::Stop => State::Idle,
        Command::Pause if state == State::Running => State::Paused,
        Command::Pause => state,
        Command::Resume if state == State::Paused => State::Running,
        Command::Resume => state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_through_states() {
        let s = apply(State::Idle, Command::Start);
        let s = apply(s, Command::Pause);
        assert_eq!(s, State::Paused);
        assert_eq!(apply(s, Command::Resume), State::Running);
        assert_eq!(apply(State::Running, Command::Stop), State::Idle);
    }
}
