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
/// Files and logical bytes in a root that share an exact size with another eligible file: the
/// space the candidate selection draws from before any cap (invariant 4: never a guess).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DuplicateCandidateSpace {
    pub files: Count,
    pub bytes: Bytes,
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
    /// Absent is unknown, never zero (an optional property that is never `null`, like `min_bytes`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_space: Option<DuplicateCandidateSpace>,
    /// Entries whose content was read and compared with at least one other file. Absent is unknown,
    /// never zero (same additive shape as `candidate_space`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entries_compared: Option<Count>,
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
            // Additive: reports from before the field existed stay readable (unknown).
            #[serde(default, deserialize_with = "crate::types::optional_present")]
            candidate_space: Option<DuplicateCandidateSpace>,
            #[serde(default, deserialize_with = "crate::types::optional_present")]
            entries_compared: Option<Count>,
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
        if fields
            .entries_compared
            .is_some_and(|compared| compared.get() > count as u64)
        {
            return Err(serde::de::Error::custom(
                "entries_compared cannot exceed the listed entries",
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
            candidate_space: fields.candidate_space,
            entries_compared: fields.entries_compared,
        })
    }
}
