use std::time::Duration;

use crate::error::{Error, Result};
use crate::writer::ComponentId;

/// How long each VSS step may take by default.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

/// Whether VSS writers take part in a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Consistency {
    /// Writers, such as SQL Server's, flush their data and briefly hold writes while the
    /// snapshot is taken, so their files are consistent. This is slower, and pauses those
    /// applications for a moment.
    #[default]
    Application,
    /// No writers take part. Files look as they would after a power cut. This is faster, and
    /// doesn't touch running applications.
    Crash,
}

/// How to create a `SnapshotSet`.
///
/// Every snapshot is a VSS *copy* backup, which tells writers not to treat it as a backup of
/// record. A database won't truncate its logs because of one, so it never disturbs another
/// backup tool.
///
/// ```
/// # #[cfg(windows)] {
/// use std::time::Duration;
/// use vss_snapshot::{Consistency, SnapshotOptions};
///
/// let options = SnapshotOptions::new()
///     .with_consistency(Consistency::Crash)
///     .with_timeout(Duration::from_secs(60));
/// assert_eq!(options.consistency(), Consistency::Crash);
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotOptions {
    consistency: Consistency,
    timeout: Duration,
    components: Vec<ComponentId>,
}

impl SnapshotOptions {
    /// Application-consistent, with up to 10 minutes for each VSS step, and no components.
    pub fn new() -> SnapshotOptions {
        SnapshotOptions {
            consistency: Consistency::Application,
            timeout: DEFAULT_TIMEOUT,
            components: Vec::new(),
        }
    }

    /// Sets whether writers take part.
    pub fn with_consistency(mut self, consistency: Consistency) -> SnapshotOptions {
        self.consistency = consistency;
        self
    }

    /// Sets the longest to wait for each VSS step, such as writers freezing. It must be more
    /// than zero.
    pub fn with_timeout(mut self, timeout: Duration) -> SnapshotOptions {
        self.timeout = timeout;
        self
    }

    /// Sets the writer components to back up, replacing any set before, so that the backup
    /// can be restored through the writer. Finish such a backup with
    /// `SnapshotSet::complete`.
    ///
    /// This needs [`Consistency::Application`], and every volume holding the components'
    /// files, which [`Component::volumes`](crate::Component::volumes) lists. Without
    /// components, writers still make their files consistent, but the backup can't be
    /// restored through them.
    pub fn with_components<I>(mut self, components: I) -> SnapshotOptions
    where
        I: IntoIterator<Item = ComponentId>,
    {
        self.components = components.into_iter().collect();
        self
    }

    /// Whether writers take part.
    pub fn consistency(&self) -> Consistency {
        self.consistency
    }

    /// The longest to wait for each VSS step.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// The writer components to back up.
    pub fn components(&self) -> &[ComponentId] {
        &self.components
    }

    /// Checks the options can be used together, before anything touches VSS.
    pub(crate) fn validate(&self) -> Result<()> {
        if self.timeout.is_zero() {
            return Err(Error::InvalidOptions {
                reason: "the timeout must be more than zero",
            });
        }
        if !self.components.is_empty() && self.consistency == Consistency::Crash {
            return Err(Error::InvalidOptions {
                reason: "components need application consistency",
            });
        }
        Ok(())
    }
}

impl Default for SnapshotOptions {
    fn default() -> SnapshotOptions {
        SnapshotOptions::new()
    }
}

/// How to run a `Restore`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreOptions {
    timeout: Duration,
}

impl RestoreOptions {
    /// Up to 10 minutes for each VSS step.
    pub fn new() -> RestoreOptions {
        RestoreOptions {
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Sets the longest to wait for each VSS step, such as writers preparing. It must be more
    /// than zero.
    pub fn with_timeout(mut self, timeout: Duration) -> RestoreOptions {
        self.timeout = timeout;
        self
    }

    /// The longest to wait for each VSS step.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Checks the options before anything touches VSS.
    pub(crate) fn validate(&self) -> Result<()> {
        if self.timeout.is_zero() {
            return Err(Error::InvalidOptions {
                reason: "the timeout must be more than zero",
            });
        }
        Ok(())
    }
}

impl Default for RestoreOptions {
    fn default() -> RestoreOptions {
        RestoreOptions::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guid::Guid;

    fn sales() -> ComponentId {
        ComponentId::new(Guid::from_u128(1), r"HOST\SQLEXPRESS", "sales")
    }

    #[test]
    fn defaults_to_application_consistency_and_ten_minutes() {
        let options = SnapshotOptions::default();
        assert_eq!(options, SnapshotOptions::new());
        assert_eq!(options.consistency(), Consistency::Application);
        assert_eq!(options.timeout(), Duration::from_secs(600));
        assert!(options.components().is_empty());
        assert!(options.validate().is_ok());
        assert_eq!(
            RestoreOptions::default().timeout(),
            Duration::from_secs(600)
        );
    }

    #[test]
    fn builders_set_each_option() {
        let options = SnapshotOptions::new()
            .with_consistency(Consistency::Crash)
            .with_timeout(Duration::from_secs(5));
        assert_eq!(options.consistency(), Consistency::Crash);
        assert_eq!(options.timeout(), Duration::from_secs(5));
        let restore = RestoreOptions::new().with_timeout(Duration::from_secs(5));
        assert_eq!(restore.timeout(), Duration::from_secs(5));
    }

    #[test]
    fn components_replace_earlier_ones() {
        let other = ComponentId::new(Guid::from_u128(2), "", "other");
        let options = SnapshotOptions::new()
            .with_components([other])
            .with_components(vec![sales()]);
        assert_eq!(options.components(), [sales()]);
    }

    #[test]
    fn components_need_application_consistency() {
        let options = SnapshotOptions::new()
            .with_consistency(Consistency::Crash)
            .with_components([sales()]);
        assert!(matches!(
            options.validate(),
            Err(Error::InvalidOptions { .. })
        ));
        let options = options.with_consistency(Consistency::Application);
        assert!(options.validate().is_ok());
    }

    #[test]
    fn a_zero_timeout_is_refused() {
        let options = SnapshotOptions::new().with_timeout(Duration::ZERO);
        assert!(matches!(
            options.validate(),
            Err(Error::InvalidOptions { .. })
        ));
        let restore = RestoreOptions::new().with_timeout(Duration::ZERO);
        assert!(matches!(
            restore.validate(),
            Err(Error::InvalidOptions { .. })
        ));
    }
}
