/// A unit of scheduled work.
pub struct Job {
    pub name: String,
    pub retries: u32,
}

fn job_names(jobs: Vec<Job>) -> Vec<String> {
    jobs.into_iter().map(|j| j.name).collect()
}

/// Returns `"a, b: N retries"` for a list of jobs.
pub fn summarize(jobs: Vec<Job>) -> String {
    let names = job_names(jobs);
    let retries: u32 = jobs.iter().map(|j| j.retries).sum();
    format!("{}: {} retries", names.join(", "), retries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_jobs() {
        let jobs = vec![
            Job { name: "build".into(), retries: 1 },
            Job { name: "deploy".into(), retries: 2 },
        ];
        assert_eq!(summarize(jobs), "build, deploy: 3 retries");
    }
}
