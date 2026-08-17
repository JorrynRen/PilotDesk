//! 房间状态机：IDLE → RUNNING ⇄ PAUSED → FINISHED / ABORTED。
//! RUNNING 内子阶段：DISCUSS（发言权串行）⇄ EXECUTE（任务并行）。

/// 房间顶层状态（与 `groupchat_rooms.status` 一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomStatus {
    Idle,
    Running,
    Paused,
    Finished,
    Aborted,
}

impl RoomStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            RoomStatus::Idle => "idle",
            RoomStatus::Running => "running",
            RoomStatus::Paused => "paused",
            RoomStatus::Finished => "finished",
            RoomStatus::Aborted => "aborted",
        }
    }

    #[allow(dead_code)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "running" => RoomStatus::Running,
            "paused" => RoomStatus::Paused,
            "finished" => RoomStatus::Finished,
            "aborted" => RoomStatus::Aborted,
            _ => RoomStatus::Idle,
        }
    }
}

/// RUNNING 内的子阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubPhase {
    Discuss,
    #[allow(dead_code)]
    Execute,
}

pub struct RoomStateMachine {
    pub status: RoomStatus,
    pub sub_phase: SubPhase,
    pub round: i64,
}

impl RoomStateMachine {
    pub fn new(status: RoomStatus) -> Self {
        Self { status, sub_phase: SubPhase::Discuss, round: 0 }
    }

    pub fn start(&mut self) {
        self.status = RoomStatus::Running;
        self.sub_phase = SubPhase::Discuss;
        self.round = 0;
    }

    #[allow(dead_code)]
    pub fn pause(&mut self) -> bool {
        if self.status == RoomStatus::Running {
            self.status = RoomStatus::Paused;
            true
        } else {
            false
        }
    }

    #[allow(dead_code)]
    pub fn resume(&mut self) -> bool {
        if self.status == RoomStatus::Paused {
            self.status = RoomStatus::Running;
            true
        } else {
            false
        }
    }

    #[allow(dead_code)]
    pub fn finish(&mut self) {
        self.status = RoomStatus::Finished;
    }

    #[allow(dead_code)]
    pub fn abort(&mut self) {
        self.status = RoomStatus::Aborted;
    }

    pub fn next_round(&mut self) {
        self.round += 1;
    }
}
