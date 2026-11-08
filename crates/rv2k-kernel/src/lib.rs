#![no_std]

use rv2k_api::{TaskId, TaskInfo, TaskState};

#[repr(C)]
#[derive(Clone, Copy)]
struct Task {
    saved_sp: usize,
    state: TaskState,
    exit_code: i16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Selection {
    Run(TaskId, usize),
    Idle,
    Finished,
}

#[derive(Clone, Copy)]
pub enum Action {
    Yield,
    Block,
    Exit(i16),
}

/// The architecture owns interrupt exclusion and context frames. All mutations
/// occur through a unique borrow while running on the kernel stack.
pub struct Scheduler {
    tasks: [Task; 2],
    current: Option<TaskId>,
    last: TaskId,
}

impl Scheduler {
    pub const fn new() -> Self {
        Self {
            tasks: [Task {
                saved_sp: 0,
                state: TaskState::Ready,
                exit_code: 0,
            }; 2],
            current: None,
            last: TaskId::Application,
        }
    }

    /// Install initial context frames before selecting the first task.
    pub fn initialize(&mut self, frames: [usize; 2]) {
        *self = Self::new();
        for (task, sp) in self.tasks.iter_mut().zip(frames) {
            task.saved_sp = sp;
        }
    }

    pub fn current(&self) -> Option<TaskId> {
        self.current
    }

    pub fn info(&self, id: TaskId) -> TaskInfo {
        let task = &self.tasks[id.index()];
        TaskInfo {
            version: 1,
            id,
            state: task.state,
            reserved: 0,
            exit_code: task.exit_code,
        }
    }

    pub fn suspend(&mut self, sp: usize, action: Action) {
        if let Some(id) = self.current.take() {
            let task = &mut self.tasks[id.index()];
            task.saved_sp = sp;
            task.state = match action {
                Action::Yield => TaskState::Ready,
                Action::Block => TaskState::Blocked,
                Action::Exit(code) => {
                    task.exit_code = code;
                    TaskState::Exited
                }
            };
            self.last = id;
        }
    }

    pub fn wake(&mut self, id: TaskId) -> bool {
        let task = &mut self.tasks[id.index()];
        if task.state != TaskState::Blocked {
            return false;
        }
        task.state = TaskState::Ready;
        true
    }

    pub fn select_next(&mut self) -> Selection {
        if let Some(id) = self.current {
            return Selection::Run(id, self.tasks[id.index()].saved_sp);
        }
        for offset in 1..=2 {
            let index = (self.last.index() + offset) % 2;
            if self.tasks[index].state == TaskState::Ready {
                let id = TaskId::from_index(index).unwrap();
                self.tasks[index].state = TaskState::Running;
                self.current = Some(id);
                return Selection::Run(id, self.tasks[index].saved_sp);
            }
        }
        if self
            .tasks
            .iter()
            .all(|task| task.state == TaskState::Exited)
        {
            Selection::Finished
        } else {
            Selection::Idle
        }
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TaskId::{Application as B, Network as A};

    #[test]
    fn round_robin_exit_and_only_ready() {
        let mut s = Scheduler::new();
        s.initialize([0x100, 0x200]);
        assert_eq!(s.select_next(), Selection::Run(A, 0x100));
        assert_eq!(s.select_next(), Selection::Run(A, 0x100));
        s.suspend(0x110, Action::Yield);
        assert_eq!(s.select_next(), Selection::Run(B, 0x200));
        s.suspend(0x210, Action::Exit(-7));
        assert_eq!(s.select_next(), Selection::Run(A, 0x110));
        for _ in 0..8 {
            s.suspend(0x120, Action::Yield);
            assert_eq!(s.select_next(), Selection::Run(A, 0x120));
            assert_eq!(s.info(B).state, TaskState::Exited);
        }
        s.suspend(0x130, Action::Exit(0));
        assert_eq!(s.select_next(), Selection::Finished);
        assert_eq!(s.info(B).exit_code, -7);
        assert!(!s.wake(B));
    }

    #[test]
    fn all_blocked_idle_wake_and_exit() {
        let mut s = Scheduler::new();
        s.initialize([0x100, 0x200]);
        s.select_next();
        s.suspend(0x110, Action::Block);
        assert_eq!(s.select_next(), Selection::Run(B, 0x200));
        s.suspend(0x210, Action::Block);
        assert_eq!(s.select_next(), Selection::Idle);
        assert!(s.wake(A));
        assert!(!s.wake(A));
        assert_eq!(s.select_next(), Selection::Run(A, 0x110));
        s.suspend(0x120, Action::Exit(9));
        assert_eq!(s.select_next(), Selection::Idle);
        assert!(s.wake(B));
        assert_eq!(s.select_next(), Selection::Run(B, 0x210));
        s.suspend(0x220, Action::Exit(0));
        assert_eq!(s.select_next(), Selection::Finished);
    }
}
