use super::super::*;
use content::{PoolError, PoolRun, host_pools};

fn hosts(n: usize) -> Vec<ContentHost> {
    (0..n)
        .map(|i| content::content_host("CDN", &format!("h{i}"), "mandatory"))
        .collect()
}

#[test]
fn pools_keep_host_order() {
    let hosts = hosts(9);
    let pools: Vec<Vec<&str>> = host_pools(&hosts)
        .map(|p| p.iter().map(|h| h.host.as_str()).collect())
        .collect();
    assert_eq!(
        pools,
        [
            vec!["h0", "h1", "h2", "h3"],
            vec!["h4", "h5", "h6", "h7"],
            vec!["h8"]
        ]
    );
}

/// Simulate the pool loop offline.
fn run_pools(outcomes: Vec<std::result::Result<u32, PoolError>>) -> (Result<u32>, usize) {
    let mut run = PoolRun::default();
    let mut tried = 0;
    for outcome in outcomes {
        tried += 1;
        if let Some(done) = run.step(outcome) {
            return (done, tried);
        }
    }
    (Err(run.exhausted()), tried)
}

#[test]
fn failed_pool_falls_back_to_next() {
    let (result, tried) = run_pools(vec![Err(PoolError::Next(anyhow!("first"))), Ok(7), Ok(8)]);
    assert_eq!(result.unwrap(), 7);
    assert_eq!(tried, 2);
}

#[test]
fn stop_error_ends_without_fallback() {
    let (result, tried) = run_pools(vec![Err(PoolError::Stop(anyhow!("fatal"))), Ok(1)]);
    assert_eq!(result.unwrap_err().to_string(), "fatal");
    assert_eq!(tried, 1);
}

#[test]
fn all_pools_failing_keeps_last_error() {
    let (result, tried) = run_pools(vec![
        Err(PoolError::Next(anyhow!("a"))),
        Err(PoolError::Next(anyhow!("b"))),
    ]);
    assert_eq!(result.unwrap_err().to_string(), "b");
    assert_eq!(tried, 2);
    let (result, _) = run_pools(Vec::new());
    assert!(result.unwrap_err().to_string().contains("no usable"));
}
