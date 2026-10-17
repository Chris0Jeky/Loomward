use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DuplicateBudget {
    pub max_candidates: Int<2, 200>,
    pub max_bytes: Bytes,
    pub max_ms: Int<1, 60000>,
    #[serde(
        default,
        deserialize_with = "crate::types::optional_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub min_bytes: Option<Bytes>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DuplicatesInspectRequest {
    pub root_id: RootId,
    pub budget: DuplicateBudget,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DuplicateEntry {
    pub relative_path: BoundedStr<1, 32768>,
    pub logical_bytes: Bytes,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DuplicateSkip {
    pub entry: Int<0, 199>,
    pub reason: BoundedStr<1, 64>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DuplicateReport {
    pub entries: BoundedVec<DuplicateEntry, 0, 200>,
    pub groups: BoundedVec<UniqueVec<Int<0, 199>, 2, 200>, 0, 100>,
    pub links: BoundedVec<UniqueVec<Int<0, 199>, 1, 200>, 0, 200>,
    pub links_not_compared: Count,
    pub skipped: BoundedVec<DuplicateSkip, 0, 200>,
    pub bytes_read: Bytes,
    pub partial: bool,
    pub truncated: bool,
    pub stop: Option<BoundedStr<1, 64>>,
}

impl<'de> Deserialize<'de> for DuplicateReport {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            entries: BoundedVec<DuplicateEntry, 0, 200>,
            groups: BoundedVec<UniqueVec<Int<0, 199>, 2, 200>, 0, 100>,
            links: BoundedVec<UniqueVec<Int<0, 199>, 1, 200>, 0, 200>,
            links_not_compared: Count,
            skipped: BoundedVec<DuplicateSkip, 0, 200>,
            bytes_read: Bytes,
            partial: bool,
            truncated: bool,
            #[serde(deserialize_with = "crate::types::required_nullable")]
            stop: Option<BoundedStr<1, 64>>,
        }
        let fields = Fields::deserialize(deserializer)?;
        // semantics.md (Duplicate inspection): groups, links and skipped.entry refer to indices in
        // entries, so an index at or past entries.len() would dangle for every consumer.
        let count = fields.entries.len();
        let within = |index: &Int<0, 199>| (index.get() as usize) < count;
        let dangling = fields
            .groups
            .iter()
            .flat_map(|g| g.iter())
            .chain(fields.links.iter().flat_map(|l| l.iter()))
            .any(|index| !within(index))
            || fields.skipped.iter().any(|skip| !within(&skip.entry));
        if dangling {
            return Err(serde::de::Error::custom(
                "group, link and skipped indices must refer to entries",
            ));
        }
        Ok(Self {
            entries: fields.entries,
            groups: fields.groups,
            links: fields.links,
            links_not_compared: fields.links_not_compared,
            skipped: fields.skipped,
            bytes_read: fields.bytes_read,
            partial: fields.partial,
            truncated: fields.truncated,
            stop: fields.stop,
        })
    }
}
