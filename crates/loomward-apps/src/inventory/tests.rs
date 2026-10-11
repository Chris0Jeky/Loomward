use super::*;

struct Fake(RegistryRead);

impl RegistryReader for Fake {
    fn read(&self, _: RegistryView) -> RegistryRead {
        self.0.clone()
    }
}

fn value(kind: u32, text: &str) -> RegistryValue {
    RegistryValue::Bytes {
        kind,
        data: text
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect(),
    }
}

fn rows(values: &[(&str, RegistryValue)]) -> Vec<ProviderInventory> {
    inventory(&Fake(RegistryRead {
        state: ProviderState::Available,
        reason: None,
        entries: vec![RegistryEntry {
            key: "synthetic-key".into(),
            values: values
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        }],
    }))
}

fn known(text: &str, expandable: bool) -> Evidence<Text> {
    Evidence::Known(Text {
        value: text.into(),
        expandable,
    })
}

#[test]
fn estimated_size_is_registry_evidence_not_disk_measurement() {
    let p = rows(&[(
        "EstimatedSize",
        RegistryValue::Bytes {
            kind: 4,
            data: 1024u32.to_le_bytes().to_vec(),
        },
    )]);
    assert_eq!(
        p[0].applications[0].estimated_size_bytes,
        Evidence::Known(1048576)
    );
    assert_eq!(
        rows(&[])[0].applications[0].estimated_size_bytes,
        Evidence::Missing
    );
    let bad = rows(&[("EstimatedSize", value(1, "1024"))]);
    assert!(matches!(
        bad[0].applications[0].estimated_size_bytes,
        Evidence::Unknown { .. }
    ));
    assert_eq!(bad[0].state, ProviderState::Available);
}

#[test]
fn malformed_size_does_not_downgrade_provider_or_healthy_entry() {
    let providers = inventory(&Fake(RegistryRead {
        state: ProviderState::Available,
        reason: None,
        entries: vec![
            RegistryEntry {
                key: "synthetic-bad".into(),
                values: BTreeMap::from([("EstimatedSize".into(), value(1, "1024"))]),
            },
            RegistryEntry {
                key: "synthetic-good".into(),
                values: BTreeMap::from([(
                    "EstimatedSize".into(),
                    RegistryValue::Bytes {
                        kind: 4,
                        data: 1u32.to_le_bytes().to_vec(),
                    },
                )]),
            },
        ],
    }));
    for provider in &providers[..3] {
        assert_eq!(provider.state, ProviderState::Available);
        assert_eq!(provider.reason, None);
        assert!(
            matches!(&provider.applications[0].estimated_size_bytes, Evidence::Unknown { reason } if !reason.is_empty())
        );
        assert_eq!(
            provider.applications[1].estimated_size_bytes,
            Evidence::Known(1024)
        );
    }
}

#[test]
fn zero_registry_size_is_unknown_not_a_known_zero_byte_size() {
    let zero = rows(&[(
        "EstimatedSize",
        RegistryValue::Bytes {
            kind: 4,
            data: 0u32.to_le_bytes().to_vec(),
        },
    )]);
    assert_eq!(
        zero[0].applications[0].estimated_size_bytes,
        Evidence::Unknown {
            reason: "provider reported zero (often unset)".into()
        }
    );
    // Unset is not unreadable: an unknown zero size keeps the provider's coverage.
    assert_eq!(zero[0].state, ProviderState::Available);
    let one_kib = rows(&[(
        "EstimatedSize",
        RegistryValue::Bytes {
            kind: 4,
            data: 1u32.to_le_bytes().to_vec(),
        },
    )]);
    assert_eq!(
        one_kib[0].applications[0].estimated_size_bytes,
        Evidence::Known(1024)
    );
}

#[test]
fn scope_and_cross_view_duplicates() {
    let providers = rows(&[("DisplayName", value(1, "Synthetic Editor"))]);
    for (index, view) in REGISTRY_VIEWS.into_iter().enumerate() {
        assert_eq!(providers[index].provider, Provider::Registry(view));
        assert_eq!(providers[index].applications.len(), 1);
        let app = &providers[index].applications[0];
        assert_eq!(app.provider, Provider::Registry(view));
        assert_eq!(
            app.scope,
            if view == RegistryView::CurrentUser {
                Scope::PerUser
            } else {
                Scope::PerMachine
            }
        );
        assert_eq!(app.display_name, known("Synthetic Editor", false));
    }
    assert_eq!(Provider::Portable.scope(), Scope::Portable);
    assert_eq!(Provider::Packaged.scope(), Scope::Unknown);
}

#[test]
fn failed_or_partial_providers_keep_coverage_and_evidence() {
    for state in [
        ProviderState::Denied,
        ProviderState::Unavailable,
        ProviderState::Partial,
    ] {
        let providers = inventory(&Fake(RegistryRead {
            state,
            reason: Some("synthetic provider failure".into()),
            entries: if state == ProviderState::Partial {
                vec![RegistryEntry {
                    key: "synthetic".into(),
                    values: BTreeMap::new(),
                }]
            } else {
                vec![]
            },
        }));
        for provider in &providers[..3] {
            assert_eq!(provider.state, state);
            assert_eq!(
                provider.reason.as_deref(),
                Some("synthetic provider failure")
            );
            assert_eq!(
                provider.applications.len(),
                usize::from(state == ProviderState::Partial)
            );
        }
    }
}

#[test]
fn denied_and_unavailable_readers_cannot_publish_entries() {
    for state in [ProviderState::Denied, ProviderState::Unavailable] {
        let providers = inventory(&Fake(RegistryRead {
            state,
            reason: Some("synthetic provider failure".into()),
            entries: vec![RegistryEntry {
                key: "synthetic".into(),
                values: BTreeMap::new(),
            }],
        }));
        for provider in &providers[..REGISTRY_VIEWS.len()] {
            assert_eq!(provider.state, state);
            assert_eq!(
                provider.reason.as_deref(),
                Some("synthetic provider failure")
            );
            assert!(provider.applications.is_empty());
        }
    }
}

#[test]
fn coverage_reasons_are_normalised_for_any_reader() {
    for state in [
        ProviderState::Available,
        ProviderState::Partial,
        ProviderState::Denied,
        ProviderState::Unavailable,
    ] {
        for reason in [None, Some(""), Some(" \t"), Some("synthetic reason")] {
            let providers = inventory(&Fake(RegistryRead {
                state,
                reason: reason.map(String::from),
                entries: Vec::new(),
            }));
            for provider in &providers[..REGISTRY_VIEWS.len()] {
                assert_eq!(provider.state, state);
                if state == ProviderState::Available {
                    assert!(provider.reason.is_none());
                } else {
                    assert!(provider
                        .reason
                        .as_ref()
                        .is_some_and(|r| !r.trim().is_empty()));
                    if reason == Some("synthetic reason") {
                        assert_eq!(provider.reason.as_deref(), reason);
                    }
                }
            }
        }
    }
}

#[test]
fn packaged_and_portable_are_explicitly_unavailable() {
    let providers = rows(&[]);
    assert_eq!(providers.len(), 5);
    for (provider, expected) in providers[3..]
        .iter()
        .zip([Provider::Packaged, Provider::Portable])
    {
        assert_eq!(provider.provider, expected);
        assert_eq!(provider.state, ProviderState::Unavailable);
        assert!(provider
            .reason
            .as_ref()
            .is_some_and(|reason| !reason.is_empty()));
        assert!(provider.applications.is_empty());
    }
}

#[test]
fn opaque_strings_and_expand_sz_are_never_executed_or_expanded() {
    let hostile = "cmd.exe /c echo synthetic & del %SYNTHETIC%\\*";
    let providers = rows(&[
        ("UninstallString", value(2, hostile)),
        ("InstallLocation", value(2, "%SYNTHETIC%\\App")),
        ("Publisher", value(1, "Synthetic Publisher")),
        ("DisplayIcon", value(1, "synthetic.dll,-7")),
        ("DisplayVersion", value(1, "1.2")),
    ]);
    let app = &providers[0].applications[0];
    assert_eq!(app.uninstall_string, known(hostile, true));
    assert_eq!(app.install_location, known("%SYNTHETIC%\\App", true));
    assert_eq!(app.publisher, known("Synthetic Publisher", false));
    assert_eq!(app.display_icon, known("synthetic.dll,-7", false));
    assert_eq!(app.display_version, known("1.2", false));
    assert_eq!(app.display_name, Evidence::Missing);
    assert_eq!(providers[0].state, ProviderState::Available);
}

#[test]
fn malformed_values_are_unknown_and_coverage_is_partial() {
    for raw in [
        RegistryValue::Bytes {
            kind: 1,
            data: vec![1],
        },
        RegistryValue::Bytes {
            kind: 1,
            data: vec![0, 216, 0, 0],
        },
        value(4, "wrong type"),
        value(1, "embedded\0nul"),
        RegistryValue::Unknown {
            reason: "read denied".into(),
        },
    ] {
        let providers = rows(&[("DisplayName", raw)]);
        assert!(matches!(
            providers[0].applications[0].display_name,
            Evidence::Unknown { .. }
        ));
        assert_eq!(providers[0].state, ProviderState::Partial);
        assert!(providers[0].reason.is_some());
    }
    // Registry strings need not include a terminating NUL (RegQueryValueExW).
    let providers = rows(&[(
        "DisplayName",
        RegistryValue::Bytes {
            kind: 1,
            data: vec![65, 0],
        },
    )]);
    assert_eq!(providers[0].applications[0].display_name, known("A", false));
}

#[test]
fn system_and_child_entries_are_retained() {
    for number in [0u32, 1] {
        let providers = rows(&[
            (
                "SystemComponent",
                RegistryValue::Bytes {
                    kind: 4,
                    data: number.to_le_bytes().to_vec(),
                },
            ),
            ("ParentKeyName", value(1, "synthetic-parent")),
        ]);
        assert_eq!(providers[0].applications.len(), 1);
        assert_eq!(
            providers[0].applications[0].system_component,
            Evidence::Known(number == 1)
        );
        assert_eq!(
            providers[0].applications[0].parent_key_name,
            known("synthetic-parent", false)
        );
    }
    for raw in [
        RegistryValue::Bytes {
            kind: 4,
            data: vec![1],
        },
        RegistryValue::Bytes {
            kind: 4,
            data: 2u32.to_le_bytes().to_vec(),
        },
        value(1, "1"),
    ] {
        let providers = rows(&[("SystemComponent", raw)]);
        assert!(matches!(
            providers[0].applications[0].system_component,
            Evidence::Unknown { .. }
        ));
        assert_eq!(providers[0].state, ProviderState::Partial);
    }
    assert_eq!(
        rows(&[])[0].applications[0].system_component,
        Evidence::Missing
    );
}

#[test]
fn runtime_source_has_no_effect_or_discovery_api() {
    for source in [
        include_str!("../inventory.rs"),
        include_str!("../lib.rs"),
        include_str!("windows.rs"),
    ] {
        for forbidden in [
            "std::process",
            "CreateProcess",
            "ShellExecute",
            "Win32_Product",
            "MsiConfigureProduct",
            "Command::",
            "std::fs",
            "std::path",
            "tokio::fs",
            "Storage::FileSystem",
            "CreateFile",
            "FindFirstFile",
            "FindNextFile",
            "GetFileAttributes",
            "ReadFile",
            "WriteFile",
            "DeleteFile",
            "MoveFile",
            "CopyFile",
            "canonicalize",
            "read_dir",
            ".exists(",
            ".metadata(",
            "symlink_metadata",
            "std::env",
            "ExpandEnvironmentStrings",
            "System::Wmi",
            "IWbem",
            "SWbem",
            "WmiQuery",
            "System::Com",
            "CoCreateInstance",
            "CoInitialize",
            "OleInitialize",
            "GetActiveObject",
            "CLSIDFrom",
        ] {
            assert!(
                !source.contains(forbidden),
                "forbidden runtime API: {forbidden}"
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn real_registry_views_report_coverage_without_recording_host_data() {
    let providers = inventory(&WindowsRegistry);
    for view in REGISTRY_VIEWS {
        let Some(provider) = providers
            .iter()
            .find(|p| p.provider == Provider::Registry(view))
        else {
            panic!("registry provider missing");
        };
        assert_eq!(
            provider.state == ProviderState::Available,
            provider.reason.is_none()
        );
    }
}
