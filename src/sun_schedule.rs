use crate::model::{
    EnvironmentAppearance, NodeSunScheduleProfile, SunScheduleAppearanceReason,
    SunScheduleAppearanceStatus,
};
use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;

const OFFICIAL_ZENITH_DEGREES: f64 = 90.833;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SunEventKind {
    Sunrise,
    Sunset,
}

pub fn missing_profile_status(evaluated_at_unix_seconds: i64) -> SunScheduleAppearanceStatus {
    SunScheduleAppearanceStatus {
        profile: None,
        evaluated_at_unix_seconds,
        local_date: None,
        sunrise_unix_seconds: None,
        sunset_unix_seconds: None,
        next_transition_unix_seconds: None,
        appearance: EnvironmentAppearance::Unknown,
        reason: SunScheduleAppearanceReason::MissingProfile,
        error: Some("sun schedule profile is required".to_string()),
    }
}

pub fn evaluate_sun_schedule(
    profile: NodeSunScheduleProfile,
    evaluated_at_unix_seconds: i64,
) -> SunScheduleAppearanceStatus {
    match evaluate_valid_profile(profile.clone(), evaluated_at_unix_seconds) {
        Ok(status) => status,
        Err((reason, error)) => SunScheduleAppearanceStatus {
            profile: Some(profile),
            evaluated_at_unix_seconds,
            local_date: None,
            sunrise_unix_seconds: None,
            sunset_unix_seconds: None,
            next_transition_unix_seconds: None,
            appearance: EnvironmentAppearance::Unknown,
            reason,
            error: Some(error),
        },
    }
}

fn evaluate_valid_profile(
    profile: NodeSunScheduleProfile,
    evaluated_at_unix_seconds: i64,
) -> Result<SunScheduleAppearanceStatus, (SunScheduleAppearanceReason, String)> {
    validate_profile(&profile)?;
    let timezone = profile.timezone.parse::<Tz>().map_err(|err| {
        (
            SunScheduleAppearanceReason::InvalidProfile,
            format!("invalid timezone '{}': {err}", profile.timezone),
        )
    })?;
    let evaluated_at = Utc
        .timestamp_opt(evaluated_at_unix_seconds, 0)
        .single()
        .ok_or_else(|| {
            (
                SunScheduleAppearanceReason::InvalidProfile,
                format!("invalid evaluated_at_unix_seconds: {evaluated_at_unix_seconds}"),
            )
        })?;
    let local_date = evaluated_at.with_timezone(&timezone).date_naive();
    let sunrise = event_for_local_date(&profile, timezone, local_date, SunEventKind::Sunrise)?;
    let sunset = event_for_local_date(&profile, timezone, local_date, SunEventKind::Sunset)?;

    let (appearance, reason) = if evaluated_at < sunrise {
        (
            EnvironmentAppearance::Dark,
            SunScheduleAppearanceReason::BeforeSunrise,
        )
    } else if evaluated_at < sunset {
        (
            EnvironmentAppearance::Light,
            SunScheduleAppearanceReason::Daylight,
        )
    } else {
        (
            EnvironmentAppearance::Dark,
            SunScheduleAppearanceReason::AfterSunset,
        )
    };
    let next_transition = if evaluated_at < sunrise {
        Some(sunrise.timestamp())
    } else if evaluated_at < sunset {
        Some(sunset.timestamp())
    } else {
        next_sunrise_after(&profile, timezone, local_date).map(|event| event.timestamp())
    };

    Ok(SunScheduleAppearanceStatus {
        profile: Some(profile),
        evaluated_at_unix_seconds,
        local_date: Some(local_date.to_string()),
        sunrise_unix_seconds: Some(sunrise.timestamp()),
        sunset_unix_seconds: Some(sunset.timestamp()),
        next_transition_unix_seconds: next_transition,
        appearance,
        reason,
        error: None,
    })
}

fn validate_profile(
    profile: &NodeSunScheduleProfile,
) -> Result<(), (SunScheduleAppearanceReason, String)> {
    if profile.node_id.trim().is_empty() {
        return Err((
            SunScheduleAppearanceReason::InvalidProfile,
            "node_id must not be empty".to_string(),
        ));
    }
    if profile.timezone.trim().is_empty() {
        return Err((
            SunScheduleAppearanceReason::InvalidProfile,
            "timezone must not be empty".to_string(),
        ));
    }
    if !profile.latitude.is_finite() || !(-90.0..=90.0).contains(&profile.latitude) {
        return Err((
            SunScheduleAppearanceReason::InvalidProfile,
            "latitude must be finite and between -90 and 90".to_string(),
        ));
    }
    if !profile.longitude.is_finite() || !(-180.0..=180.0).contains(&profile.longitude) {
        return Err((
            SunScheduleAppearanceReason::InvalidProfile,
            "longitude must be finite and between -180 and 180".to_string(),
        ));
    }
    Ok(())
}

fn next_sunrise_after(
    profile: &NodeSunScheduleProfile,
    timezone: Tz,
    local_date: NaiveDate,
) -> Option<DateTime<Utc>> {
    (1..=370)
        .filter_map(|offset| {
            event_for_local_date(
                profile,
                timezone,
                local_date.checked_add_days(chrono::Days::new(offset))?,
                SunEventKind::Sunrise,
            )
            .ok()
        })
        .next()
}

fn event_for_local_date(
    profile: &NodeSunScheduleProfile,
    timezone: Tz,
    local_date: NaiveDate,
    kind: SunEventKind,
) -> Result<DateTime<Utc>, (SunScheduleAppearanceReason, String)> {
    for offset in -1..=1 {
        let Some(utc_date) = local_date.checked_add_signed(chrono::Duration::days(offset)) else {
            continue;
        };
        let event = event_for_utc_date(profile, utc_date, kind)?;
        if event.with_timezone(&timezone).date_naive() == local_date {
            return Ok(event);
        }
    }
    Err((
        SunScheduleAppearanceReason::InvalidProfile,
        format!("could not map {kind:?} to local date {local_date}"),
    ))
}

fn event_for_utc_date(
    profile: &NodeSunScheduleProfile,
    utc_date: NaiveDate,
    kind: SunEventKind,
) -> Result<DateTime<Utc>, (SunScheduleAppearanceReason, String)> {
    let day = f64::from(utc_date.ordinal());
    let longitude_hour = profile.longitude / 15.0;
    let approximate_time = match kind {
        SunEventKind::Sunrise => day + ((6.0 - longitude_hour) / 24.0),
        SunEventKind::Sunset => day + ((18.0 - longitude_hour) / 24.0),
    };
    let mean_anomaly = (0.9856 * approximate_time) - 3.289;
    let true_longitude = normalize_degrees(
        mean_anomaly
            + (1.916 * sin_degrees(mean_anomaly))
            + (0.020 * sin_degrees(2.0 * mean_anomaly))
            + 282.634,
    );
    let mut right_ascension =
        normalize_degrees((0.91764 * tan_degrees(true_longitude)).atan().to_degrees());
    let longitude_quadrant = (true_longitude / 90.0).floor() * 90.0;
    let ascension_quadrant = (right_ascension / 90.0).floor() * 90.0;
    right_ascension = (right_ascension + longitude_quadrant - ascension_quadrant) / 15.0;

    let sin_declination = 0.39782 * sin_degrees(true_longitude);
    let cos_declination = sin_declination.asin().cos();
    let cos_hour_angle = (cos_degrees(OFFICIAL_ZENITH_DEGREES)
        - (sin_declination * sin_degrees(profile.latitude)))
        / (cos_declination * cos_degrees(profile.latitude));

    if cos_hour_angle > 1.0 {
        return Err((
            SunScheduleAppearanceReason::PolarNight,
            format!("sun never rises on {utc_date} at this latitude"),
        ));
    }
    if cos_hour_angle < -1.0 {
        return Err((
            SunScheduleAppearanceReason::PolarDay,
            format!("sun never sets on {utc_date} at this latitude"),
        ));
    }

    let hour_angle = match kind {
        SunEventKind::Sunrise => 360.0 - cos_hour_angle.acos().to_degrees(),
        SunEventKind::Sunset => cos_hour_angle.acos().to_degrees(),
    } / 15.0;
    let local_mean_time = hour_angle + right_ascension - (0.06571 * approximate_time) - 6.622;
    let utc_hour = normalize_hours(local_mean_time - longitude_hour);
    let utc_seconds = (utc_hour * 3600.0).round() as i64;
    let midnight = utc_date
        .and_hms_opt(0, 0, 0)
        .expect("midnight is valid")
        .and_utc()
        .timestamp();
    Utc.timestamp_opt(midnight + utc_seconds, 0)
        .single()
        .ok_or_else(|| {
            (
                SunScheduleAppearanceReason::InvalidProfile,
                format!("invalid computed {kind:?} timestamp for {utc_date}"),
            )
        })
}

fn normalize_degrees(value: f64) -> f64 {
    value.rem_euclid(360.0)
}

fn normalize_hours(value: f64) -> f64 {
    value.rem_euclid(24.0)
}

fn sin_degrees(value: f64) -> f64 {
    value.to_radians().sin()
}

fn cos_degrees(value: f64) -> f64 {
    value.to_radians().cos()
}

fn tan_degrees(value: f64) -> f64 {
    value.to_radians().tan()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn racter_profile() -> NodeSunScheduleProfile {
        NodeSunScheduleProfile {
            node_id: "racter".to_string(),
            timezone: "America/Los_Angeles".to_string(),
            latitude: 37.7749,
            longitude: -122.4194,
        }
    }

    fn shrdlu_profile() -> NodeSunScheduleProfile {
        NodeSunScheduleProfile {
            node_id: "shrdlu".to_string(),
            timezone: "America/New_York".to_string(),
            latitude: 40.7128,
            longitude: -74.0060,
        }
    }

    #[test]
    fn calculates_racter_daylight_from_real_sunrise_sunset() {
        let status = evaluate_sun_schedule(racter_profile(), 1_718_992_800);

        assert_eq!(status.appearance, EnvironmentAppearance::Light);
        assert_eq!(status.reason, SunScheduleAppearanceReason::Daylight);
        assert_eq!(status.local_date.as_deref(), Some("2024-06-21"));
        assert_eq!(status.sunrise_unix_seconds, Some(1_718_974_090));
        assert_eq!(status.sunset_unix_seconds, Some(1_719_027_316));
        assert_eq!(
            status.next_transition_unix_seconds,
            status.sunset_unix_seconds
        );
    }

    #[test]
    fn calculates_racter_dark_after_sunset_with_next_sunrise() {
        let status = evaluate_sun_schedule(racter_profile(), 1_719_030_600);

        assert_eq!(status.appearance, EnvironmentAppearance::Dark);
        assert_eq!(status.reason, SunScheduleAppearanceReason::AfterSunset);
        assert_eq!(status.local_date.as_deref(), Some("2024-06-21"));
        assert_eq!(status.next_transition_unix_seconds, Some(1_719_060_505));
    }

    #[test]
    fn calculates_shrdlu_dark_before_sunrise() {
        let status = evaluate_sun_schedule(shrdlu_profile(), 1_718_955_000);

        assert_eq!(status.appearance, EnvironmentAppearance::Dark);
        assert_eq!(status.reason, SunScheduleAppearanceReason::BeforeSunrise);
        assert_eq!(status.local_date.as_deref(), Some("2024-06-21"));
        assert_eq!(status.sunrise_unix_seconds, Some(1_718_961_905));
        assert_eq!(status.sunset_unix_seconds, Some(1_719_016_258));
        assert_eq!(
            status.next_transition_unix_seconds,
            status.sunrise_unix_seconds
        );
    }

    #[test]
    fn missing_profile_fails_closed_to_unknown() {
        let status = missing_profile_status(1_718_992_800);

        assert_eq!(status.appearance, EnvironmentAppearance::Unknown);
        assert_eq!(status.reason, SunScheduleAppearanceReason::MissingProfile);
        assert!(status.profile.is_none());
    }

    #[test]
    fn invalid_timezone_fails_closed_to_unknown() {
        let status = evaluate_sun_schedule(
            NodeSunScheduleProfile {
                timezone: "Mars/Olympus".to_string(),
                ..racter_profile()
            },
            1_718_992_800,
        );

        assert_eq!(status.appearance, EnvironmentAppearance::Unknown);
        assert_eq!(status.reason, SunScheduleAppearanceReason::InvalidProfile);
        assert!(
            status
                .error
                .as_deref()
                .is_some_and(|err| err.contains("timezone"))
        );
    }
}
