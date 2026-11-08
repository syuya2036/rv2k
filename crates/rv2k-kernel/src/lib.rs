#![no_std]

pub mod diagnostics;
pub mod ipc;
pub mod memory;
pub mod services;
pub mod socket;
pub mod syscall;

use rv2k_api::{event, syscall::Error, TaskId, TaskInfo, TaskState};

#[repr(u8)]
#[derive(Clone, Copy, PartialEq)]
enum Wait {
    None,
    Sleep,
    Event,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Task {
    saved_sp: usize,
    state: TaskState,
    exit_code: i16,
    deadline: u32,
    wait_mask: u32,
    pending_events: u32,
    result: i32,
    wait: Wait,
    finite: bool,
    result_ready: bool,
}

#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<Task>() <= 32);

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

#[repr(u8)]
#[derive(Clone, Copy, PartialEq)]
enum Running {
    None,
    Network,
    Application,
}
impl Running {
    fn id(self) -> Option<TaskId> {
        match self {
            Self::None => None,
            Self::Network => Some(TaskId::Network),
            Self::Application => Some(TaskId::Application),
        }
    }
    fn from_id(id: TaskId) -> Self {
        match id {
            TaskId::Network => Self::Network,
            TaskId::Application => Self::Application,
        }
    }
}

/// The architecture owns interrupt exclusion and context frames. All mutations
/// occur through a unique borrow while running on the kernel stack.
pub struct Scheduler {
    tasks: [Task; 2],
    current: Running,
    next: TaskId,
    ticks: u64,
}

impl Scheduler {
    pub const fn new() -> Self {
        Self {
            tasks: [Task {
                saved_sp: 0,
                state: TaskState::Ready,
                exit_code: 0,
                deadline: 0,
                wait_mask: 0,
                pending_events: 0,
                result: 0,
                wait: Wait::None,
                finite: false,
                result_ready: false,
            }; 2],
            current: Running::None,
            next: TaskId::Network,
            ticks: 0,
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
        self.current.id()
    }
    /// A timer quantum switches only when another task can actually run.
    pub fn can_preempt(&self) -> bool {
        match self.current {
            Running::Network => self.tasks[1].state == TaskState::Ready,
            Running::Application => self.tasks[0].state == TaskState::Ready,
            Running::None => false,
        }
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
        if let Some(id) = self.current() {
            self.current = Running::None;
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
            self.next = if id == TaskId::Network {
                TaskId::Application
            } else {
                TaskId::Network
            };
            self.recheck(id);
            if let Action::Exit(_) = action {
                let task = &mut self.tasks[1 - id.index()];
                // Only the peer can have a resource wait dependent on this exit.
                if task.state == TaskState::Blocked
                    && task.wait == Wait::Event
                    && task.wait_mask & !event::DRIVER_POLL != 0
                {
                    task.wait = Wait::None;
                    task.state = TaskState::Ready;
                    task.result = Error::Closed.code() as i32;
                    task.result_ready = true;
                }
            }
        }
    }

    pub fn wake(&mut self, id: TaskId) -> bool {
        let task = &mut self.tasks[id.index()];
        if task.state != TaskState::Blocked {
            return false;
        }
        task.state = TaskState::Ready;
        task.wait = Wait::None;
        true
    }

    pub fn uptime(&self) -> u64 {
        self.ticks
    }

    pub fn tick(&mut self) {
        self.advance_ticks(1);
    }

    pub fn advance_ticks(&mut self, ticks: u32) {
        self.ticks = self.ticks.saturating_add(ticks as u64);
        if ticks != 0 && self.tasks[TaskId::Network.index()].state != TaskState::Exited {
            self.tasks[TaskId::Network.index()].pending_events |= event::DRIVER_POLL;
        }
        self.recheck(TaskId::Network);
        self.recheck(TaskId::Application);
    }

    pub fn sleep(&mut self, ticks: u32) -> Result<Action, Error> {
        if ticks >= 0x8000_0000 {
            return Err(Error::Invalid);
        }
        if ticks == 0 {
            return Ok(Action::Yield);
        }
        let id = self.current().ok_or(Error::Invalid)?;
        let task = &mut self.tasks[id.index()];
        task.wait = Wait::Sleep;
        task.finite = true;
        task.deadline = (self.ticks as u32).wrapping_add(ticks);
        Ok(Action::Block)
    }

    /// Called with IRQ exclusion together with suspend. recheck in suspend also
    /// closes the signal-before-BLOCKED gap for callers using the function API.
    pub fn wait_event(
        &mut self,
        mask: u32,
        timeout: u32,
        level: u32,
    ) -> Result<Option<u32>, Error> {
        if mask == 0
            || mask & !event::ALL != 0
            || (timeout >= 0x8000_0000 && timeout != event::FOREVER)
        {
            return Err(Error::Invalid);
        }
        let id = self.current().ok_or(Error::Invalid)?;
        let peer_exited = self.tasks[1 - id.index()].state == TaskState::Exited;
        let task = &mut self.tasks[id.index()];
        let bits = (task.pending_events | level) & mask;
        if bits != 0 {
            task.pending_events &= !bits;
            return Ok(Some(bits));
        }
        if peer_exited && mask & !event::DRIVER_POLL != 0 {
            return Err(Error::Closed);
        }
        if timeout == 0 {
            return Err(Error::Timeout);
        }
        task.wait = Wait::Event;
        task.wait_mask = mask;
        task.finite = timeout != event::FOREVER;
        task.deadline = (self.ticks as u32).wrapping_add(timeout);
        Ok(None)
    }

    pub fn signal(&mut self, id: TaskId, mask: u32) -> Result<(), Error> {
        if mask == 0 || mask & !event::ALL != 0 {
            return Err(Error::Invalid);
        }
        if self.tasks[id.index()].state == TaskState::Exited {
            return Err(Error::Closed);
        }
        self.tasks[id.index()].pending_events |= mask;
        self.recheck(id);
        Ok(())
    }

    /// The architecture consumes this once when restoring a newly woken frame.
    pub fn take_result(&mut self, id: TaskId) -> Option<isize> {
        let task = &mut self.tasks[id.index()];
        if !task.result_ready {
            return None;
        }
        task.result_ready = false;
        Some(task.result as isize)
    }

    fn recheck(&mut self, id: TaskId) {
        let task = &mut self.tasks[id.index()];
        if task.state != TaskState::Blocked || task.wait == Wait::None {
            return;
        }
        let bits = task.pending_events & task.wait_mask;
        let result = if task.wait == Wait::Event && bits != 0 {
            task.pending_events &= !bits;
            bits as i32
        } else if task.finite && (self.ticks as u32).wrapping_sub(task.deadline) < 0x8000_0000 {
            if task.wait == Wait::Sleep {
                0
            } else {
                Error::Timeout.code() as i32
            }
        } else {
            return;
        };
        task.wait = Wait::None;
        task.state = TaskState::Ready;
        task.result = result;
        task.result_ready = true;
    }

    pub fn select_next(&mut self) -> Selection {
        if let Some(id) = self.current() {
            return Selection::Run(id, self.tasks[id.index()].saved_sp);
        }
        for offset in 0..2 {
            let index = (self.next.index() + offset) % 2;
            if self.tasks[index].state == TaskState::Ready {
                let id = TaskId::from_index(index).unwrap();
                self.tasks[index].state = TaskState::Running;
                self.current = Running::from_id(id);
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

#[cfg(test)]
mod wait_tests {
    use super::*;
    use TaskId::{Application as B, Network as A};
    fn kernel() -> Scheduler {
        let mut s = Scheduler::new();
        s.initialize([0x100, 0x200]);
        s.select_next();
        s
    }
    #[test]
    fn resource_wait_after_peer_exit_is_closed_but_ready_data_wins() {
        let mut s = kernel();
        s.suspend(0x110, Action::Exit(0));
        s.select_next();
        for timeout in [0, 5, event::FOREVER] {
            assert_eq!(
                s.wait_event(event::MSG_AVAILABLE, timeout, 0),
                Err(Error::Closed)
            );
            assert_eq!(s.info(B).state, TaskState::Running);
        }
        assert_eq!(
            s.wait_event(event::MSG_AVAILABLE, event::FOREVER, event::MSG_AVAILABLE),
            Ok(Some(event::MSG_AVAILABLE))
        );
        s.signal(B, event::NET_RX).unwrap();
        assert_eq!(
            s.wait_event(event::NET_RX, event::FOREVER, 0),
            Ok(Some(event::NET_RX))
        );
        assert_eq!(s.wait_event(event::DRIVER_POLL, 1, 0), Ok(None));
        s.suspend(0x210, Action::Block);
        s.tick();
        assert_eq!(s.take_result(B), Some(Error::Timeout.code()));
    }
    #[test]
    fn tick_wrap_sleep_zero_and_timeout_limits() {
        let mut s = kernel();
        s.ticks = u32::MAX as u64 - 1;
        assert!(matches!(s.sleep(0), Ok(Action::Yield)));
        assert!(matches!(s.sleep(0x8000_0000), Err(Error::Invalid)));
        let action = s.sleep(3).unwrap();
        s.suspend(0x110, action);
        s.tick();
        s.tick();
        assert_eq!(s.uptime(), 0x1_0000_0000);
        assert_eq!(s.info(A).state, TaskState::Blocked);
        s.tick();
        assert_eq!(s.info(A).state, TaskState::Ready);
        assert_eq!(s.take_result(A), Some(0));
        assert_eq!(s.take_result(A), None);
    }
    #[test]
    fn notification_before_during_and_after_block() {
        for timing in 0..3 {
            let mut s = kernel();
            if timing == 0 {
                s.signal(A, event::NET_RX).unwrap();
            }
            let result = s.wait_event(event::NET_RX, 5, 0).unwrap();
            if timing == 0 {
                assert_eq!(result, Some(event::NET_RX));
            } else {
                assert_eq!(result, None);
                if timing == 1 {
                    s.signal(A, event::NET_RX).unwrap();
                }
                s.suspend(0x110, Action::Block);
                if timing == 2 {
                    s.signal(A, event::NET_RX).unwrap();
                }
                assert_eq!(s.info(A).state, TaskState::Ready);
                assert_eq!(s.take_result(A), Some(event::NET_RX as isize));
            }
            s.select_next();
            if s.current() == Some(B) {
                s.suspend(0x210, Action::Yield);
                s.select_next();
            }
            assert_eq!(s.wait_event(event::NET_RX, 0, 0), Err(Error::Timeout));
        }
    }
    #[test]
    fn event_priority_sticky_bits_infinite_wait_and_exit_cancel() {
        let mut s = kernel();
        s.signal(A, event::NET_RX | event::NET_TX_READY).unwrap();
        assert_eq!(s.wait_event(event::NET_RX, 0, 0), Ok(Some(event::NET_RX)));
        assert_eq!(
            s.wait_event(event::NET_TX_READY, 0, 0),
            Ok(Some(event::NET_TX_READY))
        );
        assert_eq!(s.wait_event(0, 3, 0), Err(Error::Invalid));
        assert_eq!(s.wait_event(1 << 31, 3, 0), Err(Error::Invalid));
        assert_eq!(
            s.wait_event(event::NET_RX, 0x8000_0000, 0),
            Err(Error::Invalid)
        );
        s.wait_event(event::NET_RX, 1, 0).unwrap();
        // At the expiry tick, an already pending event beats the timeout.
        s.signal(A, event::NET_RX).unwrap();
        s.tick();
        s.suspend(0x110, Action::Block);
        assert_eq!(s.take_result(A), Some(event::NET_RX as isize));
        s.select_next();
        s.suspend(0x210, Action::Yield);
        s.select_next();
        s.wait_event(event::MSG_AVAILABLE, event::FOREVER, 0)
            .unwrap();
        s.suspend(0x110, Action::Block);
        for _ in 0..16 {
            s.tick();
        }
        assert_eq!(s.info(A).state, TaskState::Blocked);
        s.select_next();
        s.suspend(0x210, Action::Exit(0));
        assert_eq!(s.info(A).state, TaskState::Ready);
        assert_eq!(s.take_result(A), Some(Error::Closed.code()));
    }
    #[test]
    fn finite_timeout_once_and_late_event_is_preserved() {
        let mut s = kernel();
        s.wait_event(event::NET_RX, 1, 0).unwrap();
        s.suspend(0x110, Action::Block);
        s.tick();
        s.signal(A, event::NET_RX).unwrap();
        assert_eq!(s.take_result(A), Some(Error::Timeout.code()));
        s.select_next();
        s.suspend(0x210, Action::Yield);
        s.select_next();
        assert_eq!(s.wait_event(event::NET_RX, 0, 0), Ok(Some(event::NET_RX)));
    }
    #[test]
    fn preemption_eligibility_tracks_idle_block_wake_and_exit() {
        let mut s = Scheduler::new();
        s.initialize([0x100, 0x200]);
        assert!(!s.can_preempt());
        assert_eq!(s.select_next(), Selection::Run(A, 0x100));
        assert!(s.can_preempt());
        s.wait_event(event::NET_RX, event::FOREVER, 0).unwrap();
        s.suspend(0x110, Action::Block);
        assert!(!s.can_preempt());
        assert_eq!(s.select_next(), Selection::Run(B, 0x200));
        assert!(!s.can_preempt());
        s.signal(A, event::NET_RX).unwrap();
        assert!(s.can_preempt());
        s.suspend(0x210, Action::Exit(0));
        assert_eq!(s.select_next(), Selection::Run(A, 0x110));
        assert!(!s.can_preempt());
    }
}
