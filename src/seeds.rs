//! Where a node address comes from, for both binaries.
//!
//! None are compiled in: this tool exists partly because the harness it replaces curled two
//! hardcoded addresses that would rot in a repository. The flags and the config file are
//! unioned, and listening adds to that at startup.

use std::collections::BTreeMap;
use std::time::Duration;

/// Endpoints and the cluster name, from the flags and an optional config file. The file has
/// the same `[cluster]` block a node reads, so an operator points atlas at the node's own
/// configuration rather than restating it.
pub fn from_flags_and_file(
    nodes: &[String],
    config: Option<&std::path::Path>,
) -> anyhow::Result<(Vec<String>, Option<String>)> {
    let mut out = nodes.to_vec();
    let mut cluster = None;
    if let Some(path) = config {
        #[derive(serde::Deserialize)]
        struct File {
            cluster: Option<Section>,
        }
        #[derive(serde::Deserialize)]
        struct Section {
            name: Option<String>,
            #[serde(default)]
            join: Vec<String>,
            advertise: Option<String>,
        }
        let file: File = toml::from_str(&std::fs::read_to_string(path)?)?;
        if let Some(section) = file.cluster {
            cluster = section.name;
            out.extend(section.advertise);
            out.extend(section.join);
        }
    }
    let mut out: Vec<String> = out.iter().map(|e| normalise(e)).collect();
    out.sort();
    out.dedup();
    Ok((out, cluster))
}

/// One spelling per address: a scheme when none was given, no trailing slash. A bare
/// `host:port` is what an operator types, and a client refuses it without a scheme.
pub fn normalise(endpoint: &str) -> String {
    let e = endpoint.trim().trim_end_matches('/');
    if e.contains("://") {
        e.to_string()
    } else {
        format!("http://{e}")
    }
}

/// Listen once for announcements and fold what was heard into the seed list.
///
/// Listening, never announcing. Returns the clusters that are not ours separately, because no
/// node can report them: each drops a name mismatch in silence.
pub fn add_heard(
    endpoints: &mut Vec<String>,
    cluster: Option<&str>,
    window: Duration,
) -> (BTreeMap<String, String>, Option<String>) {
    match crate::discovery::listen(cluster, window) {
        Ok(heard) => {
            endpoints.extend(heard.ours.into_values().map(|e| normalise(&e)));
            endpoints.sort();
            endpoints.dedup();
            (heard.foreign, None)
        }
        // A node on this host already holds the port, on the machine most likely to be watched
        // from. Report it as text rather than swallowing it: the caller decides where it goes.
        Err(e) => (
            BTreeMap::new(),
            Some(format!("no multicast listening ({e}); seeds only")),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An operator types `host:port`; a client refuses it without a scheme. One spelling
    /// per address is also what lets two sources of the same node be told apart.
    #[test]
    fn a_bare_host_and_port_gets_a_scheme_and_loses_its_slash() {
        assert_eq!(normalise("localhost:11435"), "http://localhost:11435");
        assert_eq!(
            normalise("http://192.0.2.10:11435/"),
            "http://192.0.2.10:11435"
        );
        assert_eq!(normalise(" https://a:1 "), "https://a:1");
    }

    /// Flags and the file are unioned and deduplicated, and the cluster name comes from the
    /// file. A node listed in both must appear once.
    #[test]
    fn flags_and_file_are_unioned() {
        let dir = std::env::temp_dir().join("atlas-seeds-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("cluster.toml");
        std::fs::write(
            &path,
            "[cluster]\nname = \"home\"\njoin = [\"http://192.0.2.2:11435\", \
             \"http://192.0.2.1:11435\"]\nadvertise = \"http://192.0.2.1:11435\"\n",
        )
        .expect("write");
        let (out, cluster) =
            from_flags_and_file(&["http://192.0.2.3:11435".into()], Some(&path)).expect("parse");
        assert_eq!(cluster.as_deref(), Some("home"));
        assert_eq!(
            out,
            vec![
                "http://192.0.2.1:11435",
                "http://192.0.2.2:11435",
                "http://192.0.2.3:11435"
            ]
        );
        std::fs::remove_file(&path).ok();
    }

    /// No file: the flags stand alone and no cluster name is invented.
    #[test]
    fn flags_alone_carry_no_cluster_name() {
        let (out, cluster) = from_flags_and_file(&["http://192.0.2.9:11435".into()], None)
            .expect("no file is not an error");
        assert_eq!(out, vec!["http://192.0.2.9:11435"]);
        assert!(cluster.is_none());
    }
}
