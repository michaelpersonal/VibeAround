use axum::Json;

/// GET /api/profiles -- list saved profiles and the CLI targets each can launch.
pub async fn list_profiles_handler() -> Json<Vec<crate::api_types::ProfileLaunchOption>> {
    let profiles = common::profiles::schema::list()
        .into_iter()
        .map(common::profiles::normalize_legacy_profile)
        .map(|profile| crate::api_types::ProfileLaunchOption {
            id: profile.id,
            label: profile.label,
            provider: profile.provider,
            launch_targets: common::profiles::runtime::launch_targets_for_api_types(
                &profile.api_types,
            )
            .into_iter()
            .map(
                |(id, label, api_type)| crate::api_types::ProfileLaunchTarget {
                    id: id.to_string(),
                    label: label.to_string(),
                    api_type: api_type.to_string(),
                },
            )
            .collect(),
        })
        .collect();
    Json(profiles)
}
