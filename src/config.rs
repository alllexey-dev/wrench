//! The YAML test-case configuration: limits, memory-mapped IO and reports.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::report::{ReportConf, ReportSlice};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Purely descriptive.
    #[serde(default)]
    pub name: Option<String>,
    /// The maximum number of instructions to execute.
    pub limit: i64,
    /// Memory size in bytes.
    pub memory_size: i64,
    /// Stream address to its input values.
    #[serde(default, deserialize_with = "io_map")]
    pub memory_mapped_io: Option<BTreeMap<i64, Vec<i64>>>,
    #[serde(default)]
    pub reports: Option<Vec<ReportConf>>,
    #[serde(default)]
    pub seed: Option<i64>,
    #[serde(default)]
    pub zero_memory_init: Option<bool>,
}

/// Keys as decimal or hex; values numbers or strings, a string standing for its characters.
fn io_map<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<BTreeMap<i64, Vec<i64>>>, D::Error> {
    use serde::de::Error;
    use serde_yaml::Value;
    let Some(m) = Option::<serde_yaml::Mapping>::deserialize(d)? else {
        return Ok(None);
    };
    let mut out = BTreeMap::new();
    for (k, v) in m {
        let addr = match &k {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => crate::hs::read_int(s),
            _ => None,
        }
        .ok_or_else(|| D::Error::custom(format!("bad memory_mapped_io address: {:?}", k)))?;
        let items = match v {
            Value::Sequence(xs) => xs,
            Value::Null => Vec::new(),
            other => return Err(D::Error::custom(format!("expected a list of inputs, got {:?}", other))),
        };
        let mut values = Vec::new();
        for x in items {
            match x {
                Value::Number(n) => values.push(n.as_i64().unwrap_or_else(|| n.as_f64().unwrap_or(0.0).round() as i64)),
                Value::String(s) => values.extend(s.chars().map(|c| c as i64)),
                _ => return Err(D::Error::custom("Expected a Char, String, or Int")),
            }
        }
        out.insert(addr, values);
    }
    Ok(Some(out))
}

impl Default for Config {
    fn default() -> Self {
        Config {
            name: None,
            limit: 1000,
            memory_size: 512,
            memory_mapped_io: None,
            reports: Some(vec![ReportConf {
                name: Some("Executed Instruction Log".to_string()),
                slice: ReportSlice::All,
                assert: None,
                view: Some("{pc}: {instruction:next} {pc:label}\n".to_string()),
            }]),
            seed: None,
            zero_memory_init: Some(false),
        }
    }
}

impl Config {
    /// Fill what the file left out from the defaults.
    fn with_defaults(mut self) -> Self {
        let d = Config::default();
        self.reports = self.reports.or(d.reports);
        self.zero_memory_init = self.zero_memory_init.or(d.zero_memory_init);
        self
    }

    pub fn parse(text: &str) -> Result<Config, String> {
        serde_yaml::from_str::<Config>(text)
            .map(Config::with_defaults)
            .map_err(|e| e.to_string())
    }

    pub fn read(path: &str) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path, e))?;
        Config::parse(&text)
    }

    /// Pick a random seed if the configuration has none.
    pub fn ensure_seed(mut self) -> Self {
        if self.seed.is_none() {
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);
            let mut g = crate::rng::StdGen::new(t ^ std::process::id() as i64);
            self.seed = Some(g.uniform_r(0, i64::MAX));
        }
        self
    }

    /// Append the built-in `Overview` report (`--stats`).
    pub fn with_execution_stats(mut self) -> Self {
        let mut rs = self.reports.unwrap_or_default();
        rs.push(execution_stats_report());
        self.reports = Some(rs);
        self
    }
}

pub fn execution_stats_report() -> ReportConf {
    ReportConf {
        name: Some("Overview".to_string()),
        slice: ReportSlice::Last,
        assert: None,
        view: Some(
            [
                "## Execution statistics",
                "sim:instruction-count: {sim:instruction-count}",
                "mem:accesses-total:    {mem:accesses-total} (reads {mem:reads-total} / writes {mem:writes-total})",
                "layout:sections-size:  {layout:sections-size} (text {layout:text-sections-size} / data {layout:data-sections-size})",
                "mem:instr-ranges:      {mem:instr-ranges}",
                "mem:data-ranges:       {mem:data-ranges}",
                "mem:io-ranges:         {mem:io-ranges}",
                "{isa-specific}",
                "",
                "## Memory layout",
                "```",
                "{mem:table}",
                "```",
                "",
                "## Memory accesses per instruction",
                "```",
                "{mem:per-instruction}",
                "```",
            ]
            .iter()
            .map(|l| format!("{}\n", l))
            .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_config() {
        let c =
            Config::parse("limit: 10\nmemory_size: 0x90\nmemory_mapped_io:\n  0x80: [5, 'ab']\n  132: []\n").unwrap();
        assert_eq!(c.memory_size, 144);
        let io = c.memory_mapped_io.unwrap();
        assert_eq!(io[&128], vec![5, 97, 98]);
        assert_eq!(io[&132], Vec::<i64>::new());
        assert!(c.reports.is_some());
    }

    #[test]
    fn config_samples() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/test/golden/config/");
        let read = |f: &str| Config::read(&format!("{}{}", dir, f));
        assert!(read("bad_no_limit.yaml").unwrap_err().contains("missing field `limit`"));
        assert!(read("bad_no_memory_size.yaml")
            .unwrap_err()
            .contains("missing field `memory_size`"));
        assert!(read("bad_too_much_limit.yaml")
            .unwrap_err()
            .contains("missing field `memory_size`"));
        assert!(read("bad_unknown_field.yaml")
            .unwrap_err()
            .contains("unknown field `filter`"));
        let strict = read("only_strict.yaml").unwrap();
        assert_eq!((strict.limit, strict.memory_size), (900, 144));
        assert_eq!(strict.reports, Config::default().reports);
        let smoke = read("smoke.yaml").unwrap();
        assert_eq!(smoke.seed, Some(1337));
        assert_eq!(
            smoke.memory_mapped_io.unwrap()[&99],
            vec![1, 2, 3, 104, 101, 108, 108, 111]
        );
        let slices: Vec<ReportSlice> = smoke.reports.unwrap().into_iter().map(|r| r.slice).collect();
        assert_eq!(
            slices,
            vec![
                ReportSlice::Last,
                ReportSlice::All,
                ReportSlice::Head(10),
                ReportSlice::Tail(10)
            ]
        );
    }

    #[test]
    fn bad_configs_are_rejected() {
        assert!(Config::parse("memory_size: 10\n").is_err());
        assert!(Config::parse("limit: 10\n").is_err());
        assert!(Config::parse("limit: 10\nmemory_size: 10\nfoo: 1\n").is_err());
    }
}
