/// The backup components document VSS produces when a backup of selected components
/// completes, from `SnapshotSet::complete`.
///
/// Store it with the backup. A restore through the writer, with `Restore::begin`, can't
/// happen without it. It's XML text, and this crate doesn't look inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupDocument {
    xml: String,
}

impl BackupDocument {
    /// Wraps a document read back from storage. It isn't checked until it's used.
    pub fn from_xml(xml: impl Into<String>) -> BackupDocument {
        BackupDocument { xml: xml.into() }
    }

    /// The document's XML, to store.
    pub fn as_xml(&self) -> &str {
        &self.xml
    }

    /// The document's XML, to store.
    pub fn into_xml(self) -> String {
        self.xml
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_xml_exactly() {
        let xml = "<BACKUP_COMPONENTS xmlns=\"x-schema:#VssComponentMetadata\"/>";
        let document = BackupDocument::from_xml(xml);
        assert_eq!(document.as_xml(), xml);
        assert_eq!(document.into_xml(), xml);
    }
}
