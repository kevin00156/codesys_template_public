//! Phase-1 interim trajectory generation: acceleration-limited velocity ramps
//! and a trapezoidal point-to-point profile.
//!
//! This is a stand-in. Phase 2 replaces the trapezoid with rsruckig
//! (jerk-limited S-curves matching MC_MoveAbsolute/MoveVelocity + Jerk
//! semantics); the call sites in `axis.rs` are shaped so only this module
//! changes.

/// Move `current` toward `target` at at most `rate` per second.
pub fn ramp_vel(current: f64, target: f64, rate: f64, dt: f64) -> f64 {
    let dv = target - current;
    let max = rate.abs() * dt;
    if dv.abs() <= max {
        target
    } else {
        current + max.copysign(dv)
    }
}

/// One tick of a trapezoidal move toward `target`.
/// Returns `(new_pos, new_vel, done)`.
pub fn trapezoid_tick(
    pos: f64,
    vel: f64,
    target: f64,
    vmax: f64,
    acc: f64,
    dec: f64,
    dt: f64,
) -> (f64, f64, bool) {
    let dist = target - pos;
    if dist == 0.0 && vel == 0.0 {
        return (target, 0.0, true);
    }
    let dir = if dist >= 0.0 { 1.0 } else { -1.0 };

    // Highest speed from which we can still stop at the target with `dec`.
    let v_stop = (2.0 * dec.abs() * dist.abs()).sqrt();
    let v_des = dir * v_stop.min(vmax.abs());
    // Speeding up (|vel| growing, or starting from rest) uses acc; slowing
    // down uses dec — including the braking half of a reversal, where
    // v_des already points the other way.
    let dv = v_des - vel;
    let rate = if vel == 0.0 || dv * vel > 0.0 { acc } else { dec };
    let vel = ramp_vel(vel, v_des, rate, dt);
    let new_pos = pos + vel * dt;

    // Terminal snap: we passed the target (discretisation overshoot is
    // bounded by one tick on the braking curve) or we are within one tick of
    // it — land exactly.
    let passed = (target - new_pos) * dir <= 0.0;
    let within_one_tick = dist.abs() <= vel.abs() * dt;
    if passed || within_one_tick {
        return (target, 0.0, true);
    }
    (new_pos, vel, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_saturates_and_lands() {
        let mut v = 0.0;
        v = ramp_vel(v, 10.0, 100.0, 0.01); // 1.0 per tick
        assert!((v - 1.0).abs() < 1e-12);
        for _ in 0..20 {
            v = ramp_vel(v, 10.0, 100.0, 0.01);
        }
        assert_eq!(v, 10.0, "must land exactly on target");
    }

    #[test]
    fn trapezoid_reaches_target_and_respects_vmax() {
        let (mut p, mut v) = (0.0, 0.0);
        let mut peak: f64 = 0.0;
        let mut ticks = 0;
        loop {
            let (np, nv, done) = trapezoid_tick(p, v, 10.0, 5.0, 50.0, 50.0, 0.001);
            p = np;
            v = nv;
            peak = peak.max(v);
            ticks += 1;
            assert!(ticks < 20_000, "did not converge: p={p} v={v}");
            if done {
                break;
            }
        }
        assert_eq!(p, 10.0);
        assert_eq!(v, 0.0);
        assert!(peak <= 5.0 + 1e-9, "vmax exceeded: {peak}");
        // 10 units at ≤5/s with ramps: at least 2s of ticks
        assert!(ticks >= 2000, "too fast to be physical: {ticks} ticks");
    }

    #[test]
    fn trapezoid_negative_direction() {
        let (mut p, mut v) = (3.0, 0.0);
        for _ in 0..20_000 {
            let (np, nv, done) = trapezoid_tick(p, v, -7.0, 20.0, 100.0, 100.0, 0.001);
            p = np;
            v = nv;
            if done {
                break;
            }
            assert!(nv <= 0.0, "wrong direction");
        }
        assert_eq!(p, -7.0);
    }

    /// Retarget behind the axis while cruising: the reversal brakes with
    /// `dec` (not `acc`), never jumps, and still lands exactly.
    #[test]
    fn trapezoid_reversal_brakes_with_dec_and_lands() {
        let (acc, dec, dt) = (100.0, 400.0, 0.001);
        let (mut p, mut v) = (0.0, 10.0); // cruising +10, now told to go to -5
        let mut ticks_to_zero = 0;
        loop {
            let (np, nv, _) = trapezoid_tick(p, v, -5.0, 10.0, acc, dec, dt);
            assert!((np - p).abs() <= 10.0 * dt + 1e-12, "position jump {p} → {np}");
            p = np;
            v = nv;
            if v <= 0.0 {
                break;
            }
            ticks_to_zero += 1;
            assert!(ticks_to_zero < 1000, "never reversed");
        }
        // 10 → 0 at dec=400 is 25 ms; braking with acc=100 would take 100 ms.
        assert!(
            ticks_to_zero <= 26,
            "reversal braked with acc, not dec: {ticks_to_zero} ticks"
        );
        for _ in 0..100_000 {
            let (np, nv, done) = trapezoid_tick(p, v, -5.0, 10.0, acc, dec, dt);
            p = np;
            v = nv;
            if done {
                break;
            }
        }
        assert_eq!(p, -5.0);
        assert_eq!(v, 0.0);
    }
}
