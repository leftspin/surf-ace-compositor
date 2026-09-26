use crate::model::{EnvironmentAppearance, NodeSunScheduleProfile};
use std::fs;
use std::path::Path;
use std::process::Command;

const HOSTNAME_PATH: &str = "/etc/hostname";
const LOCALTIME_PATH: &str = "/etc/localtime";
const ZONEINFO_ROOT: &str = "/usr/share/zoneinfo";
const ZONE1970_TAB_PATH: &str = "/usr/share/zoneinfo/zone1970.tab";
pub const DESKTOP_COLOR_SCHEME_SOURCE: &str = "gsettings org.gnome.desktop.interface color-scheme";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DesktopColorSchemePreference {
    pub appearance: Option<EnvironmentAppearance>,
    pub source_available: bool,
}

impl DesktopColorSchemePreference {
    pub fn status_appearance(self) -> Option<EnvironmentAppearance> {
        self.source_available
            .then_some(self.appearance.unwrap_or(EnvironmentAppearance::Unknown))
    }

    pub fn status_source(self) -> Option<String> {
        self.source_available
            .then(|| DESKTOP_COLOR_SCHEME_SOURCE.to_string())
    }
}

pub fn discover_host_sun_schedule_profile(
    node_id_override: Option<&str>,
) -> Result<NodeSunScheduleProfile, String> {
    discover_host_sun_schedule_profile_from(
        node_id_override,
        Path::new(HOSTNAME_PATH),
        Path::new(LOCALTIME_PATH),
        Path::new(ZONEINFO_ROOT),
        Path::new(ZONE1970_TAB_PATH),
    )
}

fn discover_host_sun_schedule_profile_from(
    node_id_override: Option<&str>,
    hostname_path: &Path,
    localtime_path: &Path,
    zoneinfo_root: &Path,
    zone1970_tab_path: &Path,
) -> Result<NodeSunScheduleProfile, String> {
    let (node_id, node_id_source, override_source) = match node_id_override.map(str::trim) {
        Some(node_id) if !node_id.is_empty() => (
            node_id.to_string(),
            "deployment override".to_string(),
            Some("--sun-schedule-node or SURF_ACE_COMPOSITOR_SUN_SCHEDULE_NODE".to_string()),
        ),
        _ => {
            let node_id = fs::read_to_string(hostname_path)
                .map_err(|err| format!("failed to read {}: {err}", hostname_path.display()))?
                .trim()
                .to_string();
            if node_id.is_empty() {
                return Err(format!(
                    "{} contains an empty hostname",
                    hostname_path.display()
                ));
            }
            (node_id, hostname_path.display().to_string(), None)
        }
    };

    let timezone = timezone_from_localtime(localtime_path, zoneinfo_root)?;
    let (latitude, longitude) = coordinates_for_timezone(zone1970_tab_path, &timezone)?;
    Ok(NodeSunScheduleProfile {
        node_id,
        timezone,
        latitude,
        longitude,
        node_id_source: Some(node_id_source),
        timezone_source: Some(localtime_path.display().to_string()),
        coordinates_source: Some(zone1970_tab_path.display().to_string()),
        override_source,
    })
}

fn timezone_from_localtime(localtime_path: &Path, zoneinfo_root: &Path) -> Result<String, String> {
    let target = fs::canonicalize(localtime_path)
        .map_err(|err| format!("failed to resolve {}: {err}", localtime_path.display()))?;
    let root = fs::canonicalize(zoneinfo_root)
        .map_err(|err| format!("failed to resolve {}: {err}", zoneinfo_root.display()))?;
    let relative = target.strip_prefix(&root).map_err(|_| {
        format!(
            "{} resolves outside {}: {}",
            localtime_path.display(),
            zoneinfo_root.display(),
            target.display()
        )
    })?;
    let timezone = relative
        .to_str()
        .ok_or_else(|| format!("timezone path is not valid UTF-8: {}", relative.display()))?
        .trim_matches('/')
        .to_string();
    if timezone.is_empty() || timezone.starts_with("posix/") || timezone.starts_with("right/") {
        return Err(format!(
            "{} does not resolve to a canonical IANA timezone: {}",
            localtime_path.display(),
            target.display()
        ));
    }
    Ok(timezone)
}

fn coordinates_for_timezone(path: &Path, timezone: &str) -> Result<(f64, f64), String> {
    let table = fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    for line in table.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut fields = line.split('\t');
        let _countries = fields.next();
        let coordinates = fields.next();
        let zone = fields.next();
        if zone == Some(timezone) {
            return parse_iso6709_coordinates(coordinates.ok_or_else(|| {
                format!("missing coordinates for {timezone} in {}", path.display())
            })?);
        }
    }
    Err(format!(
        "timezone {timezone} has no coordinate entry in {}",
        path.display()
    ))
}

fn parse_iso6709_coordinates(value: &str) -> Result<(f64, f64), String> {
    let split = value
        .char_indices()
        .skip(1)
        .find_map(|(index, ch)| matches!(ch, '+' | '-').then_some(index))
        .ok_or_else(|| format!("invalid ISO 6709 coordinate pair: {value}"))?;
    let latitude = parse_iso6709_component(&value[..split], 2)?;
    let longitude = parse_iso6709_component(&value[split..], 3)?;
    Ok((latitude, longitude))
}

fn parse_iso6709_component(value: &str, degree_digits: usize) -> Result<f64, String> {
    let sign = match value.as_bytes().first() {
        Some(b'+') => 1.0,
        Some(b'-') => -1.0,
        _ => return Err(format!("coordinate component lacks a sign: {value}")),
    };
    let digits = &value[1..];
    if digits.len() != degree_digits + 2 && digits.len() != degree_digits + 4 {
        return Err(format!("invalid coordinate component: {value}"));
    }
    let degrees = digits[..degree_digits]
        .parse::<f64>()
        .map_err(|err| format!("invalid degrees in {value}: {err}"))?;
    let minutes = digits[degree_digits..degree_digits + 2]
        .parse::<f64>()
        .map_err(|err| format!("invalid minutes in {value}: {err}"))?;
    let seconds = if digits.len() == degree_digits + 4 {
        digits[degree_digits + 2..]
            .parse::<f64>()
            .map_err(|err| format!("invalid seconds in {value}: {err}"))?
    } else {
        0.0
    };
    if minutes >= 60.0 || seconds >= 60.0 {
        return Err(format!("coordinate component is out of range: {value}"));
    }
    Ok(sign * (degrees + minutes / 60.0 + seconds / 3600.0))
}

pub fn discover_desktop_color_scheme() -> DesktopColorSchemePreference {
    let output = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
        .ok();
    let Some(output) = output.filter(|output| output.status.success()) else {
        return DesktopColorSchemePreference::default();
    };
    let Ok(value) = String::from_utf8(output.stdout) else {
        return DesktopColorSchemePreference::default();
    };
    DesktopColorSchemePreference {
        appearance: parse_desktop_color_scheme(&value),
        source_available: true,
    }
}

fn parse_desktop_color_scheme(value: &str) -> Option<EnvironmentAppearance> {
    match value.trim().trim_matches('\'').trim_matches('"') {
        "prefer-dark" => Some(EnvironmentAppearance::Dark),
        "prefer-light" => Some(EnvironmentAppearance::Light),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should follow epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("surf-ace-{label}-{nonce}"))
    }

    #[test]
    fn discovers_hostname_timezone_coordinates_and_explicit_sources() {
        let root = temp_root("host-profile");
        let zoneinfo = root.join("zoneinfo");
        let zone = zoneinfo.join("America/Los_Angeles");
        fs::create_dir_all(zone.parent().expect("zone should have parent"))
            .expect("zone directory should be created");
        fs::write(root.join("hostname"), "racter\n").expect("hostname should write");
        fs::write(&zone, b"test zone").expect("zone should write");
        symlink(&zone, root.join("localtime")).expect("localtime should link");
        fs::write(
            root.join("zone1970.tab"),
            "US\t+340308-1181434\tAmerica/Los_Angeles\tPacific\n",
        )
        .expect("zone table should write");

        let profile = discover_host_sun_schedule_profile_from(
            None,
            &root.join("hostname"),
            &root.join("localtime"),
            &zoneinfo,
            &root.join("zone1970.tab"),
        )
        .expect("host profile should resolve");

        assert_eq!(profile.node_id, "racter");
        assert_eq!(profile.timezone, "America/Los_Angeles");
        assert!((profile.latitude - 34.052_222).abs() < 0.000_001);
        assert!((profile.longitude - -118.242_778).abs() < 0.000_001);
        assert_eq!(
            profile.node_id_source.as_deref(),
            Some(root.join("hostname").to_string_lossy().as_ref())
        );
        assert_eq!(
            profile.timezone_source.as_deref(),
            Some(root.join("localtime").to_string_lossy().as_ref())
        );
        assert_eq!(
            profile.coordinates_source.as_deref(),
            Some(root.join("zone1970.tab").to_string_lossy().as_ref())
        );
        assert!(profile.override_source.is_none());

        fs::remove_dir_all(root).expect("test root should be removed");
    }

    #[test]
    fn node_override_changes_identity_only_and_records_provenance() {
        let root = temp_root("host-profile-override");
        let zoneinfo = root.join("zoneinfo");
        let zone = zoneinfo.join("America/New_York");
        fs::create_dir_all(zone.parent().expect("zone should have parent"))
            .expect("zone directory should be created");
        fs::write(root.join("hostname"), "ignored\n").expect("hostname should write");
        fs::write(&zone, b"test zone").expect("zone should write");
        symlink(&zone, root.join("localtime")).expect("localtime should link");
        fs::write(
            root.join("zone1970.tab"),
            "US\t+404251-0740023\tAmerica/New_York\tEastern (most areas)\n",
        )
        .expect("zone table should write");

        let profile = discover_host_sun_schedule_profile_from(
            Some("exception-node"),
            &root.join("hostname"),
            &root.join("localtime"),
            &zoneinfo,
            &root.join("zone1970.tab"),
        )
        .expect("override profile should resolve");

        assert_eq!(profile.node_id, "exception-node");
        assert_eq!(profile.timezone, "America/New_York");
        assert!(profile.override_source.is_some());

        fs::remove_dir_all(root).expect("test root should be removed");
    }

    #[test]
    fn desktop_color_scheme_maps_only_explicit_gnome_preferences() {
        assert_eq!(
            parse_desktop_color_scheme("'prefer-light'\n"),
            Some(EnvironmentAppearance::Light)
        );
        assert_eq!(
            parse_desktop_color_scheme("'prefer-dark'\n"),
            Some(EnvironmentAppearance::Dark)
        );
        assert_eq!(parse_desktop_color_scheme("'default'\n"), None);
        assert_eq!(parse_desktop_color_scheme("'other'\n"), None);

        let default = DesktopColorSchemePreference {
            appearance: parse_desktop_color_scheme("'default'\n"),
            source_available: true,
        };
        assert_eq!(
            default.status_appearance(),
            Some(EnvironmentAppearance::Unknown)
        );
        assert_eq!(
            default.status_source().as_deref(),
            Some(DESKTOP_COLOR_SCHEME_SOURCE)
        );

        let unavailable = DesktopColorSchemePreference::default();
        assert_eq!(unavailable.status_appearance(), None);
        assert_eq!(unavailable.status_source(), None);
    }

    #[test]
    fn parses_zone1970_coordinate_variants() {
        assert_eq!(
            parse_iso6709_coordinates("+4042-07400").expect("minute coordinates should parse"),
            (40.7, -74.0)
        );
        let (latitude, longitude) =
            parse_iso6709_coordinates("+340308-1181434").expect("second coordinates should parse");
        assert!((latitude - 34.052_222).abs() < 0.000_001);
        assert!((longitude - -118.242_778).abs() < 0.000_001);
    }
}
