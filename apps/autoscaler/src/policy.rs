//! 인스턴스 수를 정하는 규칙. 외부(docker)와 상관없는 순수한 계산만 둬서 단위 테스트한다.
//!
//! Kubernetes HPA와 같은 식을 쓴다.
//! - 원하는 수 = ceil(현재 수 × 평균 CPU ÷ 목표 CPU), 최소·최대로 자른다.
//! - 평균 CPU가 목표와 허용 오차 이내로 가까우면 그대로 둔다(작은 흔들림에 반응하지 않는다).
//! - 늘리기는 바로 한다. 줄이기는 안정화 시간 동안의 원하는 수 중 가장 큰 값으로 한다
//!   (부하가 잠깐 내려갔다 다시 오를 때 줄였다 늘렸다 하지 않는다).

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy)]
pub struct Policy {
    pub min: usize,
    pub max: usize,
    /// 인스턴스 하나의 목표 CPU 사용률(%). 코어 1개를 다 쓰면 100%다.
    pub target_cpu: f64,
    /// 목표 대비 비율이 1에서 이만큼 이내면 그대로 둔다. 예: 0.1이면 목표의 90~110%.
    pub tolerance: f64,
}

impl Policy {
    /// 지금 인스턴스 수와 평균 CPU 사용률로 원하는 인스턴스 수를 정한다.
    pub fn desired_replicas(&self, current: usize, average_cpu: f64) -> usize {
        if current == 0 {
            return self.min.max(1).min(self.max);
        }
        let ratio = average_cpu.max(0.0) / self.target_cpu;
        let desired = if (ratio - 1.0).abs() <= self.tolerance {
            current
        } else {
            (current as f64 * ratio).ceil() as usize
        };
        desired.clamp(self.min, self.max)
    }
}

/// 줄이기를 안정화 시간만큼 미룬다.
pub struct Stabilizer {
    window: Duration,
    history: VecDeque<(Instant, usize)>,
}

impl Stabilizer {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            history: VecDeque::new(),
        }
    }

    /// 이번 원하는 수를 기록하고, 실제로 맞출 인스턴스 수를 돌려준다.
    /// 늘리는 쪽이면 바로 그 수를, 줄이는 쪽이면 안정화 시간 동안의 원하는 수 중 가장 큰 값을 준다.
    pub fn recommend(&mut self, now: Instant, current: usize, desired: usize) -> usize {
        self.history.push_back((now, desired));
        while let Some(&(at, _)) = self.history.front() {
            if now.duration_since(at) > self.window {
                self.history.pop_front();
            } else {
                break;
            }
        }
        if desired >= current {
            return desired;
        }
        let highest = self
            .history
            .iter()
            .map(|&(_, desired)| desired)
            .max()
            .unwrap_or(desired);
        highest.min(current)
    }
}

/// 줄일 때 고를 수 있는지 보려고 각 인스턴스의 `/healthz`에서 읽은 상태.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    /// 컨테이너 이름(예: chzzk-app-3).
    pub name: String,
    /// `/healthz`의 `live_ingests`. 헬스 체크에 답하지 않았으면 `None`.
    pub live_ingests: Option<u64>,
    pub shutting_down: bool,
}

/// 줄일 인스턴스를 고른다. 송출을 받고 있지 않고 정상으로 답한 인스턴스만, 나중에 생긴 것(이름의
/// 번호가 큰 것)부터 `count`개까지 고른다. 송출 중이거나 상태를 모르는 인스턴스는 건드리지 않는다.
pub fn pick_removals(instances: &[Instance], count: usize) -> Vec<String> {
    let mut idle: Vec<&Instance> = instances
        .iter()
        .filter(|instance| instance.live_ingests == Some(0) && !instance.shutting_down)
        .collect();
    idle.sort_by_key(|instance| std::cmp::Reverse(replica_number(&instance.name)));
    idle.into_iter()
        .take(count)
        .map(|instance| instance.name.clone())
        .collect()
}

/// Compose 컨테이너 이름 끝의 번호(`chzzk-app-3` → 3). 없으면 0.
fn replica_number(name: &str) -> u64 {
    name.rsplit(['-', '_'])
        .next()
        .and_then(|number| number.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: Policy = Policy {
        min: 1,
        max: 5,
        target_cpu: 50.0,
        tolerance: 0.1,
    };

    #[test]
    fn 목표보다_바쁘면_비율만큼_늘린다() {
        assert_eq!(POLICY.desired_replicas(1, 150.0), 3);
        assert_eq!(POLICY.desired_replicas(2, 80.0), 4); // 2 × 1.6 = 3.2 → 4
    }

    #[test]
    fn 목표보다_한가하면_비율만큼_줄인다() {
        assert_eq!(POLICY.desired_replicas(4, 10.0), 1);
        assert_eq!(POLICY.desired_replicas(4, 30.0), 3); // 4 × 0.6 = 2.4 → 3
    }

    #[test]
    fn 허용_오차_안이면_그대로_둔다() {
        assert_eq!(POLICY.desired_replicas(3, 54.0), 3);
        assert_eq!(POLICY.desired_replicas(3, 46.0), 3);
        assert_eq!(POLICY.desired_replicas(3, 56.0), 4);
    }

    #[test]
    fn 최소_최대를_넘지_않는다() {
        assert_eq!(POLICY.desired_replicas(4, 900.0), 5);
        assert_eq!(POLICY.desired_replicas(2, 0.0), 1);
        let at_least_two = Policy { min: 2, ..POLICY };
        assert_eq!(at_least_two.desired_replicas(3, 0.0), 2);
    }

    #[test]
    fn 인스턴스가_없으면_최소_수로_띄운다() {
        assert_eq!(POLICY.desired_replicas(0, 0.0), 1);
        assert_eq!(Policy { min: 2, ..POLICY }.desired_replicas(0, 0.0), 2);
    }

    #[test]
    fn 늘리기는_바로_하고_줄이기는_안정화_시간_동안_미룬다() {
        let start = Instant::now();
        let mut stabilizer = Stabilizer::new(Duration::from_secs(60));

        assert_eq!(stabilizer.recommend(start, 1, 4), 4);
        // 부하가 사라져 원하는 수가 1이 되어도 60초 안에는 4를 유지한다.
        let later = start + Duration::from_secs(30);
        assert_eq!(stabilizer.recommend(later, 4, 1), 4);
        // 4를 원한 기록이 60초를 넘겨 지나가면 줄인다.
        let much_later = start + Duration::from_secs(61);
        assert_eq!(stabilizer.recommend(much_later, 4, 1), 1);
    }

    #[test]
    fn 안정화_시간_안의_가장_큰_값까지만_줄인다() {
        let start = Instant::now();
        let mut stabilizer = Stabilizer::new(Duration::from_secs(60));
        stabilizer.recommend(start, 5, 5);
        stabilizer.recommend(start + Duration::from_secs(70), 5, 3);
        assert_eq!(
            stabilizer.recommend(start + Duration::from_secs(80), 5, 1),
            3
        );
    }

    fn instance(name: &str, live_ingests: Option<u64>) -> Instance {
        Instance {
            name: name.into(),
            live_ingests,
            shutting_down: false,
        }
    }

    #[test]
    fn 송출_중이거나_상태를_모르는_인스턴스는_줄이지_않는다() {
        let instances = [
            instance("chzzk-app-1", Some(0)),
            instance("chzzk-app-2", Some(1)),
            instance("chzzk-app-3", None),
            instance("chzzk-app-4", Some(0)),
        ];
        assert_eq!(
            pick_removals(&instances, 3),
            vec!["chzzk-app-4", "chzzk-app-1"]
        );
    }

    #[test]
    fn 나중에_생긴_인스턴스부터_줄인다() {
        let instances = [
            instance("chzzk-app-2", Some(0)),
            instance("chzzk-app-10", Some(0)),
            instance("chzzk-app-9", Some(0)),
        ];
        assert_eq!(
            pick_removals(&instances, 2),
            vec!["chzzk-app-10", "chzzk-app-9"]
        );
    }

    #[test]
    fn 내려가는_중인_인스턴스는_다시_고르지_않는다() {
        let mut leaving = instance("chzzk-app-3", Some(0));
        leaving.shutting_down = true;
        assert_eq!(
            pick_removals(&[leaving, instance("chzzk-app-2", Some(0))], 1),
            vec!["chzzk-app-2"]
        );
    }
}
