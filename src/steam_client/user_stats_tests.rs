use super::*;

const SCHEMA: &str = r##""480"
{
    "gamename" "Spacewar"
    "stats"
    {
        "1" { "type" "1" "name" "NumGames" "default" "3" }
        "2" { "type" "2" "name" "FeetTraveled" "default" "1.5" }
        "3" { "type" "3" "name" "AverageSpeed" }
        "4"
        {
            "type" "4"
            "name" "ach_block"
            "bits"
            {
                "0"
                {
                    "name" "ACH_WIN_ONE_GAME"
                    "display"
                    {
                        "name" { "english" "Winner" "german" "Gewinner" "token" "#t" }
                        "desc" { "english" "Win one game" }
                        "hidden" "0"
                    }
                }
                "1"
                {
                    "name" "ACH_TRAVEL_FAR"
                    "bit" "5"
                    "display" { "name" "Far" "desc" "" "hidden" "1" }
                }
                "40" { "name" "BAD_BIT" }
            }
        }
    }
}"##;

fn schema(language: &str) -> Schema {
    parse_schema(SCHEMA.as_bytes(), language)
}

#[test]
fn schema_stats_and_defaults() {
    let s = schema("english");
    let kinds: Vec<_> = s
        .stats
        .iter()
        .map(|s| (s.id, s.name.as_str(), s.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            (1, "NumGames", 1),
            (2, "FeetTraveled", 2),
            (3, "AverageSpeed", 3)
        ]
    );
    assert_eq!(s.stats[0].value, 3);
    assert_eq!(f32::from_bits(s.stats[1].value), 1.5);
    assert_eq!(s.stats[2].value, 0f32.to_bits());
}

#[test]
fn schema_achievements_localized() {
    let s = schema("german");
    assert_eq!(s.achievements.len(), 2, "bit 40 dropped");
    let win = &s.achievements[0];
    assert_eq!(
        (win.block, win.bit, win.name.as_str()),
        (4, 0, "ACH_WIN_ONE_GAME")
    );
    assert_eq!(win.display_name, "Gewinner");
    assert_eq!(win.description, "Win one game", "english fallback");
    assert!(!win.hidden);
    let far = &s.achievements[1];
    assert_eq!(
        (far.bit, far.display_name.as_str(), far.hidden),
        (5, "Far", true)
    );
}

#[test]
fn values_override_defaults() {
    let values: HashMap<u32, u32> = [(1, 42), (2, 9.25f32.to_bits())].into_iter().collect();
    let mut times = vec![0u32; 32];
    times[5] = 1_700_000_000;
    let unlocks: HashMap<u32, Vec<u32>> = [(4, times)].into_iter().collect();
    let (stats, achievements) = merge_values(schema("english"), &values, &unlocks);
    assert_eq!(stats[0].value, 42);
    assert_eq!(f32::from_bits(stats[1].value), 9.25);
    assert_eq!(achievements[0].unlock_time, 0);
    assert_eq!(achievements[1].unlock_time, 1_700_000_000);
}

#[test]
fn empty_or_garbage_schema_is_empty() {
    assert_eq!(parse_schema(&[], "english"), Schema::default());
    assert_eq!(parse_schema(&[0; 64], "english").stats, Vec::new());
}

/// Tiny binary KeyValues writer.
enum Kv {
    Obj(&'static str, Vec<Kv>),
    Str(&'static str, &'static str),
    Int(&'static str, i32),
}

fn encode(kv: &Kv, out: &mut Vec<u8>) {
    let key = |out: &mut Vec<u8>, k: &str| {
        out.extend_from_slice(k.as_bytes());
        out.push(0);
    };
    match kv {
        Kv::Obj(k, children) => {
            out.push(0);
            key(out, k);
            for child in children {
                encode(child, out);
            }
            out.push(8);
        }
        Kv::Str(k, v) => {
            out.push(1);
            key(out, k);
            key(out, v);
        }
        Kv::Int(k, v) => {
            out.push(2);
            key(out, k);
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
}

#[test]
fn binary_schema_with_int_types() {
    use Kv::*;
    let tree = Obj(
        "480",
        vec![Obj(
            "stats",
            vec![
                Obj(
                    "7",
                    vec![Int("type", 1), Str("name", "Kills"), Int("default", 2)],
                ),
                Obj(
                    "8",
                    vec![
                        Int("type", 4),
                        Obj(
                            "bits",
                            vec![Obj("3", vec![Str("name", "ACH_X"), Int("bit", 3)])],
                        ),
                    ],
                ),
            ],
        )],
    );
    let mut bytes = Vec::new();
    encode(&tree, &mut bytes);
    bytes.push(8);
    let s = parse_schema(&bytes, "english");
    assert_eq!(s.stats.len(), 1);
    assert_eq!(
        (s.stats[0].id, s.stats[0].kind, s.stats[0].value),
        (7, 1, 2)
    );
    assert_eq!(s.achievements.len(), 1);
    assert_eq!((s.achievements[0].block, s.achievements[0].bit), (8, 3));
}
