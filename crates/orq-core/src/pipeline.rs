use std::time::Instant;

use crate::{
    BuildOptions, CacheKey, CompactReport, Compressor, CoreError, ImpactAnalyzer, ResultCache,
    StepRunner, VerifyRequest, VerifyResult,
};

/// The verification flow shared by the CLI (in-process mode) and the daemon:
/// impact → cache lookup → runner → cache store.
///
/// Cache failures never fail a verification: they are logged and the
/// pipeline carries on as if there were no cache.
pub struct Pipeline<A, R, C, P> {
    /// Impact analyzer.
    pub analyzer: A,
    /// Ladder runner.
    pub runner: R,
    /// Result cache; `None` disables caching entirely.
    pub cache: Option<C>,
    /// Report compressor.
    pub compressor: P,
    /// Feature/target selection used when a request has none.
    pub build: BuildOptions,
}

impl<A, R, C, P> Pipeline<A, R, C, P>
where
    A: ImpactAnalyzer,
    R: StepRunner,
    C: ResultCache,
    P: Compressor,
{
    /// Verifies `request` and returns the full result.
    ///
    /// When `request.changes` is `None` the changes are detected with
    /// [`ImpactAnalyzer::detect_changes`]. On a cache hit the stored result is
    /// returned with `cache_hit = true` and `total_ms` set to the time this
    /// call took.
    pub async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResult, CoreError> {
        let start = Instant::now();
        let root = &request.root;
        let changes = match &request.changes {
            Some(changes) => changes.clone(),
            None => self.analyzer.detect_changes(root)?,
        };
        let impact = self.analyzer.analyze(root, &changes)?;
        let build = request.build.as_ref().unwrap_or(&self.build);

        let cache = self.cache.as_ref().filter(|_| request.use_cache);
        let key: Option<CacheKey> = cache.and_then(|c| {
            c.key(root, &impact, request.up_to, build)
                .map_err(|e| tracing::warn!("cache key failed: {e}"))
                .ok()
        });

        if let (Some(cache), Some(key)) = (cache, &key) {
            match cache.get(key) {
                Ok(Some(mut hit)) => {
                    hit.cache_hit = true;
                    hit.impact = impact;
                    hit.total_ms = elapsed_ms(start);
                    return Ok(hit);
                }
                Ok(None) => {}
                Err(e) => tracing::warn!("cache lookup failed: {e}"),
            }
        }

        let mut result = self.runner.run(root, &impact, request.up_to, build).await?;
        result.cache_hit = false;
        result.total_ms = elapsed_ms(start);

        if let (Some(cache), Some(key)) = (cache, &key) {
            if let Err(e) = cache.put(key, &result) {
                tracing::warn!("cache store failed: {e}");
            }
        }
        Ok(result)
    }

    /// Compresses `result` for the agent.
    pub fn report(&self, result: &VerifyResult, budget: usize) -> CompactReport {
        self.compressor.compress(result, budget)
    }
}

fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use super::*;
    use crate::{ChangeSet, CrateId, ImpactSet, Outcome, Step};

    struct FakeAnalyzer;
    impl ImpactAnalyzer for FakeAnalyzer {
        fn analyze(&self, _: &Path, changes: &ChangeSet) -> Result<ImpactSet, CoreError> {
            Ok(ImpactSet {
                changed: changes
                    .files
                    .iter()
                    .map(|f| CrateId {
                        name: f.display().to_string(),
                        manifest_path: PathBuf::new(),
                    })
                    .collect(),
                ..ImpactSet::default()
            })
        }
        fn detect_changes(&self, _: &Path) -> Result<ChangeSet, CoreError> {
            Ok(ChangeSet {
                files: vec!["detected".into()],
            })
        }
    }

    #[derive(Default)]
    struct CountingRunner(AtomicUsize);
    impl StepRunner for CountingRunner {
        fn run(
            &self,
            _: &Path,
            impact: &ImpactSet,
            _: Step,
            _: &BuildOptions,
        ) -> impl Future<Output = Result<VerifyResult, CoreError>> + Send {
            self.0.fetch_add(1, Ordering::SeqCst);
            let impact = impact.clone();
            async move {
                Ok(VerifyResult {
                    outcome: Outcome::Pass,
                    steps: Vec::new(),
                    diagnostics: Vec::new(),
                    test_failures: Vec::new(),
                    impact,
                    cache_hit: false,
                    total_ms: 0,
                })
            }
        }
    }

    #[derive(Default)]
    struct MemCache(Mutex<Vec<(CacheKey, VerifyResult)>>);
    impl ResultCache for MemCache {
        fn key(
            &self,
            _: &Path,
            impact: &ImpactSet,
            up_to: Step,
            build: &BuildOptions,
        ) -> Result<CacheKey, CoreError> {
            let mut k = [0u8; 32];
            k[0] = impact.changed.len() as u8;
            k[1] = up_to as u8;
            k[2] = build.features.len() as u8;
            Ok(CacheKey(k))
        }
        fn get(&self, key: &CacheKey) -> Result<Option<VerifyResult>, CoreError> {
            let entries = self.0.lock().unwrap();
            Ok(entries
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone()))
        }
        fn put(&self, key: &CacheKey, result: &VerifyResult) -> Result<(), CoreError> {
            self.0.lock().unwrap().push((*key, result.clone()));
            Ok(())
        }
    }

    struct FailingCache;
    impl ResultCache for FailingCache {
        fn key(
            &self,
            _: &Path,
            _: &ImpactSet,
            _: Step,
            _: &BuildOptions,
        ) -> Result<CacheKey, CoreError> {
            Err(CoreError::Cache("broken".into()))
        }
        fn get(&self, _: &CacheKey) -> Result<Option<VerifyResult>, CoreError> {
            Err(CoreError::Cache("broken".into()))
        }
        fn put(&self, _: &CacheKey, _: &VerifyResult) -> Result<(), CoreError> {
            Err(CoreError::Cache("broken".into()))
        }
    }

    struct EchoCompressor;
    impl Compressor for EchoCompressor {
        fn compress(&self, result: &VerifyResult, _: usize) -> CompactReport {
            CompactReport {
                text: format!("hit={}", result.cache_hit),
                json: serde_json::Value::Null,
                estimated_tokens: 1,
                truncated: false,
            }
        }
    }

    /// Minimal executor: the fakes never return `Pending`.
    fn block_on<F: Future>(future: F) -> F::Output {
        use std::pin::pin;
        use std::task::{Context, Poll, Waker};
        let mut future = pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    fn pipeline<C: ResultCache>(
        cache: Option<C>,
    ) -> Pipeline<FakeAnalyzer, CountingRunner, C, EchoCompressor> {
        Pipeline {
            analyzer: FakeAnalyzer,
            runner: CountingRunner::default(),
            cache,
            compressor: EchoCompressor,
            build: BuildOptions::default(),
        }
    }

    fn request(files: Option<&[&str]>) -> VerifyRequest {
        VerifyRequest {
            changes: files.map(|f| ChangeSet {
                files: f.iter().map(PathBuf::from).collect(),
            }),
            ..VerifyRequest::default()
        }
    }

    #[test]
    fn second_call_hits_cache() {
        let p = pipeline(Some(MemCache::default()));
        let req = request(Some(&["a"]));
        let first = block_on(p.verify(&req)).unwrap();
        let second = block_on(p.verify(&req)).unwrap();
        assert!(!first.cache_hit);
        assert!(second.cache_hit);
        assert_eq!(p.runner.0.load(Ordering::SeqCst), 1);
        assert_eq!(p.report(&second, 100).text, "hit=true");
    }

    #[test]
    fn use_cache_false_always_runs() {
        let p = pipeline(Some(MemCache::default()));
        let req = VerifyRequest {
            use_cache: false,
            ..request(Some(&["a"]))
        };
        block_on(p.verify(&req)).unwrap();
        let again = block_on(p.verify(&req)).unwrap();
        assert!(!again.cache_hit);
        assert_eq!(p.runner.0.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn detects_changes_when_none_given() {
        let p = pipeline::<MemCache>(None);
        let result = block_on(p.verify(&request(None))).unwrap();
        assert_eq!(result.impact.changed[0].name, "detected");
    }

    #[test]
    fn request_build_options_change_the_key() {
        let p = pipeline(Some(MemCache::default()));
        let plain = request(Some(&["a"]));
        let with_features = VerifyRequest {
            build: Some(BuildOptions {
                features: vec!["x".into()],
                ..BuildOptions::default()
            }),
            ..plain.clone()
        };
        block_on(p.verify(&plain)).unwrap();
        let second = block_on(p.verify(&with_features)).unwrap();
        assert!(!second.cache_hit);
        assert_eq!(p.runner.0.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn broken_cache_does_not_fail_verification() {
        let p = pipeline(Some(FailingCache));
        let result = block_on(p.verify(&request(Some(&["a"])))).unwrap();
        assert!(result.outcome.is_pass());
        assert_eq!(p.runner.0.load(Ordering::SeqCst), 1);
    }
}
