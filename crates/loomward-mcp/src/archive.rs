use super::{service_error, Host};
use loomward_operations::manifest::MIN_RESERVE;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

// Detail inspection is bounded independently of subtree totals; an incomplete range says so.
const DETAIL_PAGES: usize = 32;

fn bytes(value: &Value) -> Option<u64> {
    value.as_str()?.parse().ok()
}
fn byte_value(value: Option<u64>) -> Value {
    value.map(|n| json!(n.to_string())).unwrap_or(Value::Null)
}

fn space(total: Option<u64>, destination: &Value) -> Value {
    let reserve =
        bytes(&destination["capacity_bytes"]).map(|capacity| MIN_RESERVE.max(capacity / 100));
    let free = bytes(&destination["free_bytes"]);
    let available = free
        .zip(reserve)
        .map(|(free, reserve)| free.saturating_sub(reserve));
    let fits = match (total, free, reserve) {
        (Some(total), Some(free), Some(reserve)) => json!(total
            .checked_add(reserve)
            .is_some_and(|needed| needed <= free)),
        _ => json!("unknown"),
    };
    json!({"reserve_bytes":byte_value(reserve),"available_after_reserve_bytes":byte_value(available),"fits":fits})
}

#[derive(Default)]
struct Details {
    modified: bool,
    attributes: bool,
    from: Option<String>,
    to: Option<String>,
    complete: bool,
    placeholder: bool,
    reparse: bool,
}

impl Details {
    fn observe(&mut self, row: &Value) {
        self.modified |= row.get("modified_at").is_some();
        self.attributes |= row.get("attributes").is_some();
        if self.modified {
            if let Some(time) = row["modified_at"].as_str() {
                if self
                    .from
                    .as_deref()
                    .is_none_or(|v| time_key(time) < time_key(v))
                {
                    self.from = Some(time.to_owned());
                }
                if self
                    .to
                    .as_deref()
                    .is_none_or(|v| time_key(time) > time_key(v))
                {
                    self.to = Some(time.to_owned());
                }
            } else {
                self.complete = false;
            }
        }
        if row["coverage"] != "complete" {
            self.complete = false;
        }
        for (key, values) in [
            (
                "attributes",
                &["recall_on_open", "recall_on_data_access"][..],
            ),
            ("flags", &["cloud_placeholder"][..]),
        ] {
            self.placeholder |= row[key].as_array().is_some_and(|a| {
                a.iter()
                    .any(|s| s.as_str().is_some_and(|s| values.contains(&s)))
            });
        }
        self.reparse |= row["attributes"]
            .as_array()
            .is_some_and(|a| a.contains(&json!("reparse_point")))
            || row["flags"]
                .as_array()
                .is_some_and(|a| a.contains(&json!("reparse_not_followed")));
    }

    fn output(&self, value: &mut Value) {
        if self.modified {
            value["modified_range"] =
                json!({"from":self.from,"to":self.to,"complete":self.complete});
        }
        if self.attributes {
            value["attribute_assessment_complete"] = self.complete.into();
        }
    }
}

// Catalogue timestamps are UTC RFC3339; compare seconds and fractional digits separately so
// an exact second sorts before that second's fractional observations (the trailing Z must not).
fn time_key(time: &str) -> (&str, &str) {
    let time = time.strip_suffix('Z').unwrap_or(time);
    time.split_once('.').unwrap_or((time, ""))
}

fn warning(warnings: &mut Vec<Value>, reference: &Value, code: &str, message: &str) {
    warnings.push(json!({"reference":reference,"code":code,"message":message}));
}

impl Host {
    pub(super) fn plan_archive(&mut self, args: &Map<String, Value>) -> Result<Value, String> {
        self.service()
            .client_archive_fields(&self.args.principal)
            .map_err(service_error)?;
        let volumes = self.read("volumes.list", json!({}))?;
        let visible = self.volumes_output(&volumes)?;
        let destination = visible["volumes"]
            .as_array()
            .expect("projected volumes")
            .iter()
            .find(|volume| volume["volume_key"] == args["destination_volume"])
            .ok_or("unknown destination volume; call volumes again")?
            .clone();
        let roots = self.read("roots.list", json!({}))?;
        let mut items = Vec::new();
        let mut warnings = Vec::new();
        let mut total_bytes = Some(0u64);
        let mut total_files = Some(0u64);
        let mut total_details = Details {
            complete: true,
            ..Details::default()
        };
        let mut freshness = Vec::new();
        let mut root_freshness = BTreeMap::new();
        let mut coverage_complete = true;
        let mut pages = DETAIL_PAGES;
        let deadline = Instant::now() + Duration::from_secs(5);
        for reference in args["references"].as_array().expect("validated") {
            let ids = self.resolve(reference.as_str().expect("validated"))?;
            let node = ids
                .node_id
                .ok_or("root has never been scanned; node is unknown")?;
            let detail = self.read("node.inspect", json!({"node_id":node}))?;
            let row = &detail["row"];
            let source = visible["volumes"]
                .as_array()
                .expect("volumes")
                .iter()
                .find(|v| v["volume_key"].as_str() == ids.volume_key.as_deref());
            let mut scan = roots["roots"]
                .as_array()
                .expect("roots")
                .iter()
                .find(|root| root["root_id"].as_str() == ids.root_id.as_deref())
                .map(|r| r["scan"].clone())
                .unwrap_or(json!({"state":"unknown","coverage":"unknown"}));
            if let Some(scan) = scan.as_object_mut() {
                scan.remove("generation");
            }
            let observed = if let Some(root) = &ids.root_id {
                if !root_freshness.contains_key(root) {
                    root_freshness.insert(root.clone(), self.root_freshness(root)?);
                }
                root_freshness[root].clone()
            } else {
                json!({"state":"unknown","at":null,"reason":null})
            };
            if !freshness.contains(&observed) {
                freshness.push(observed.clone());
            }
            let known_size = row["kind"] != "dir" || row["coverage"] == "complete";
            let item_bytes = bytes(&row["logical_bytes"]).filter(|_| known_size);
            let item_files = row["files"].as_u64().filter(|_| known_size);
            total_bytes = total_bytes
                .zip(item_bytes)
                .and_then(|(a, b)| a.checked_add(b));
            total_files = total_files
                .zip(item_files)
                .and_then(|(a, b)| a.checked_add(b));
            coverage_complete &= row["coverage"] == "complete" && scan["coverage"] == "complete";
            let mut item = json!({"reference":reference,"name":row["name"],"kind":row["kind"],"bytes":byte_value(item_bytes),"files":item_files,"coverage":row["coverage"],"scan":scan,"freshness":observed});
            if source.is_some_and(|source| source["online"] == false) {
                warning(
                    &mut warnings,
                    reference,
                    "offline_source",
                    "Source volume is offline; the catalogue is a stored observation.",
                );
            }
            if ids.volume_key.as_deref() == destination["volume_key"].as_str() {
                warning(
                    &mut warnings,
                    reference,
                    "same_volume",
                    "Destination is on the same volume as this source: not a backup.",
                );
            }
            if row["files"]
                .as_u64()
                .zip(row["dirs"].as_u64())
                .and_then(|(f, d)| f.checked_add(d))
                .and_then(|entries| entries.checked_add(u64::from(row["kind"] == "dir")))
                .is_some_and(|entries| entries > 200_000)
            {
                warning(
                    &mut warnings,
                    reference,
                    "split_item",
                    "Item exceeds 200,000 entries; split it before copying.",
                );
            }
            let mut details = Details {
                complete: true,
                ..Details::default()
            };
            details.observe(row);
            // Optional details come only from projected service rows. Never infer permission
            // from a missing value, and never walk the filesystem to fill a catalogue gap.
            if details.modified || details.attributes {
                let mut pending = VecDeque::new();
                if row["kind"] == "dir" {
                    pending.push_back((node, Value::Null));
                }
                while let Some((node, cursor)) = pending.pop_front() {
                    if pages == 0 || Instant::now() >= deadline {
                        details.complete = false;
                        break;
                    }
                    pages -= 1;
                    let page = self.read("tree.children",json!({"node_id":node,"sort":"size_desc","basis":"logical","limit":200,"cursor":cursor}))?;
                    if page["budget_hit"] == true {
                        details.complete = false;
                    }
                    for row in page["items"].as_array().ok_or("invalid detail page")? {
                        details.observe(row);
                        if row["kind"] == "dir" {
                            pending.push_back((
                                row["node_id"]
                                    .as_str()
                                    .ok_or("invalid detail node")?
                                    .to_owned(),
                                Value::Null,
                            ));
                        }
                    }
                    if !page["next_cursor"].is_null() {
                        pending.push_front((node, page["next_cursor"].clone()));
                    }
                }
            }
            if details.placeholder {
                warning(
                    &mut warnings,
                    reference,
                    "cloud_placeholder",
                    "Catalogue records cloud placeholders; content availability is unverified.",
                );
            }
            if details.reparse {
                warning(
                    &mut warnings,
                    reference,
                    "reparse_point",
                    "Catalogue records reparse points; targets are not followed.",
                );
            }
            details.output(&mut item);
            total_details.modified |= details.modified;
            total_details.attributes |= details.attributes;
            total_details.complete &= details.complete;
            if let Some(from) = details.from {
                if total_details
                    .from
                    .as_ref()
                    .is_none_or(|v| time_key(&from) < time_key(v))
                {
                    total_details.from = Some(from);
                }
            }
            if let Some(to) = details.to {
                if total_details
                    .to
                    .as_ref()
                    .is_none_or(|v| time_key(&to) > time_key(v))
                {
                    total_details.to = Some(to);
                }
            }
            items.push(item);
        }
        let mut total = json!({"bytes":byte_value(total_bytes),"files":total_files,"coverage":if coverage_complete {"complete"} else {"partial"},"freshness":freshness});
        total_details.output(&mut total);
        let mut out = json!({"items":items,"total":total,"destination":destination,"space":space(total_bytes,&destination),"warnings":warnings,"shape":"new folder only","execution":"owner_approval_required","reason":"Personal copy gate C8 is closed: docs/47 section 11 requires C6, G5, G9 and the owner's Q1-Q3 decisions and approval. MCP cannot approve or execute a copy.","totals_basis":"sum of selected items; overlapping selections count separately"});
        if let Some(name) = args.get("folder_name") {
            out["folder_name"] = name.clone();
        }
        self.check_stamp()?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_order_exact_seconds_before_fractional_times_and_keep_missing_values_unknown() {
        let mut details = Details {
            complete: true,
            ..Details::default()
        };
        for timestamp in [
            json!("2026-01-01T00:00:00.1Z"),
            json!("2026-01-01T00:00:00Z"),
            Value::Null,
        ] {
            details.observe(&json!({"modified_at":timestamp,"coverage":"complete"}));
        }
        let mut out = json!({});
        details.output(&mut out);
        assert_eq!(out["modified_range"]["from"], "2026-01-01T00:00:00Z");
        assert_eq!(out["modified_range"]["to"], "2026-01-01T00:00:00.1Z");
        assert_eq!(out["modified_range"]["complete"], false);
    }

    #[test]
    fn archive_reserve_fit_unknown_and_overflow_are_honest() {
        let destination = json!({"capacity_bytes":"1000000000000","free_bytes":"10000000009"});
        assert_eq!(space(Some(9), &destination)["fits"], true);
        assert_eq!(space(Some(10), &destination)["fits"], false);
        assert_eq!(space(Some(9), &destination)["reserve_bytes"], "10000000000");
        let small = json!({"capacity_bytes":"1000","free_bytes":(MIN_RESERVE+9).to_string()});
        assert_eq!(
            space(Some(9), &small)["reserve_bytes"],
            MIN_RESERVE.to_string()
        );
        assert_eq!(space(Some(9), &small)["available_after_reserve_bytes"], "9");
        assert_eq!(space(Some(u64::MAX), &small)["fits"], false);
        for destination in [
            json!({"capacity_bytes":null,"free_bytes":"100"}),
            json!({"capacity_bytes":"100","free_bytes":null}),
        ] {
            assert_eq!(space(Some(1), &destination)["fits"], "unknown");
            assert!(space(Some(1), &destination)["available_after_reserve_bytes"].is_null());
        }
        assert_eq!(space(None, &small)["fits"], "unknown");
        assert_eq!(
            space(Some(0), &json!({"capacity_bytes":"100","free_bytes":"0"}))["fits"],
            false
        );
    }
}
