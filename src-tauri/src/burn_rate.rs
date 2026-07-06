/// Projects milliseconds-until-100% from a series of `(ts_ms, pct)` samples
/// using ordinary least-squares slope. Returns `None` when there are fewer
/// than 2 points, the pct isn't increasing (slope <= 0 — already trending
/// down or flat, e.g. right after a window reset), or the projection would
/// land more than 30 days out (not a meaningful "burn rate" at that point).
pub fn project_time_to_limit(points: &[(i64, f64)], now_ms: i64) -> Option<i64> {
    if points.len() < 2 {
        return None;
    }
    let n = points.len() as f64;
    let mean_t: f64 = points.iter().map(|(t, _)| *t as f64).sum::<f64>() / n;
    let mean_p: f64 = points.iter().map(|(_, p)| *p).sum::<f64>() / n;
    let mut num = 0.0;
    let mut den = 0.0;
    for (t, p) in points {
        let dt = *t as f64 - mean_t;
        num += dt * (*p - mean_p);
        den += dt * dt;
    }
    if den == 0.0 {
        return None;
    }
    let slope_pct_per_ms = num / den; // pct change per millisecond
    if slope_pct_per_ms <= 0.0 {
        return None;
    }
    let latest_pct = points.last().unwrap().1;
    if latest_pct >= 100.0 {
        return Some(0);
    }
    let ms_to_100 = ((100.0 - latest_pct) / slope_pct_per_ms) as i64;
    let latest_ts = points.last().unwrap().0;
    let eta_ms = latest_ts - now_ms + ms_to_100;
    if eta_ms < 0 || ms_to_100 > 30 * 86_400_000 {
        None
    } else {
        Some(eta_ms.max(0))
    }
}

/// A rolling window resets its pct to ~0 at `resets_at` (epoch seconds), so
/// no projected ETA past that point can ever be reached — the window is gone
/// before usage would. Suppresses (returns `None`) any ETA that lands at or
/// after the window's own reset, since it's not a real risk at the current
/// burn rate.
pub fn cap_at_reset(eta_ms: Option<i64>, resets_at: Option<u64>, now_ms: i64) -> Option<i64> {
    let eta_ms = eta_ms?;
    if let Some(resets_at) = resets_at {
        let remaining_ms = resets_at as i64 * 1000 - now_ms;
        if eta_ms >= remaining_ms {
            return None;
        }
    }
    Some(eta_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fewer_than_two_points_returns_none() {
        assert_eq!(project_time_to_limit(&[(0, 10.0)], 0), None);
        assert_eq!(project_time_to_limit(&[], 0), None);
    }

    #[test]
    fn flat_usage_returns_none() {
        let points = vec![(0, 50.0), (60_000, 50.0), (120_000, 50.0)];
        assert_eq!(project_time_to_limit(&points, 120_000), None);
    }

    #[test]
    fn decreasing_usage_returns_none() {
        // A window reset mid-sample-window would show pct dropping; that's
        // not a "burn rate" — don't project a nonsensical ETA off it.
        let points = vec![(0, 80.0), (60_000, 40.0), (120_000, 10.0)];
        assert_eq!(project_time_to_limit(&points, 120_000), None);
    }

    #[test]
    fn steady_linear_increase_projects_a_sane_eta() {
        // +10% per minute starting at 50% -> 5 more minutes to 100%.
        let points = vec![(0, 50.0), (60_000, 60.0), (120_000, 70.0)];
        let eta = project_time_to_limit(&points, 120_000).unwrap();
        assert!((eta - 300_000).abs() < 5_000, "eta was {eta}ms, expected ~300000ms");
    }

    #[test]
    fn already_at_limit_returns_zero() {
        let points = vec![(0, 90.0), (60_000, 100.0)];
        assert_eq!(project_time_to_limit(&points, 60_000), Some(0));
    }

    #[test]
    fn eta_past_reset_is_suppressed() {
        // A 5h window with 1h left can't be hit by a 6.5h-out projection.
        let now_ms = 0;
        let resets_at_secs = 3_600; // 1h from now
        let eta_ms = 6 * 3_600_000 + 30 * 60_000; // 6h30m
        assert_eq!(cap_at_reset(Some(eta_ms), Some(resets_at_secs), now_ms), None);
    }

    #[test]
    fn eta_before_reset_passes_through() {
        let now_ms = 0;
        let resets_at_secs = 3_600 * 5; // 5h from now
        let eta_ms = 3_600_000; // 1h
        assert_eq!(cap_at_reset(Some(eta_ms), Some(resets_at_secs), now_ms), Some(eta_ms));
    }

    #[test]
    fn no_resets_at_passes_through_unchanged() {
        assert_eq!(cap_at_reset(Some(1_000), None, 0), Some(1_000));
    }

    #[test]
    fn none_eta_stays_none() {
        assert_eq!(cap_at_reset(None, Some(1), 0), None);
    }
}
