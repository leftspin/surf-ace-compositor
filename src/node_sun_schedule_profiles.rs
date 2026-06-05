use crate::model::NodeSunScheduleProfile;

pub const RACTER_NODE_ID: &str = "racter";
pub const SHRDLU_NODE_ID: &str = "shrdlu";

pub fn profile_for_node(node_id: &str) -> Option<NodeSunScheduleProfile> {
    match node_id {
        RACTER_NODE_ID => Some(NodeSunScheduleProfile {
            node_id: RACTER_NODE_ID.to_string(),
            timezone: "America/Los_Angeles".to_string(),
            latitude: 37.7749,
            longitude: -122.4194,
        }),
        SHRDLU_NODE_ID => Some(NodeSunScheduleProfile {
            node_id: SHRDLU_NODE_ID.to_string(),
            timezone: "America/New_York".to_string(),
            latitude: 40.7128,
            longitude: -74.0060,
        }),
        _ => None,
    }
}

pub fn supported_node_ids() -> &'static [&'static str] {
    &[RACTER_NODE_ID, SHRDLU_NODE_ID]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_owned_appliance_profiles() {
        let racter = profile_for_node("racter").expect("racter profile should exist");
        assert_eq!(racter.timezone, "America/Los_Angeles");
        assert_eq!(racter.latitude, 37.7749);
        assert_eq!(racter.longitude, -122.4194);

        let shrdlu = profile_for_node("shrdlu").expect("shrdlu profile should exist");
        assert_eq!(shrdlu.timezone, "America/New_York");
        assert_eq!(shrdlu.latitude, 40.7128);
        assert_eq!(shrdlu.longitude, -74.0060);
    }
}
