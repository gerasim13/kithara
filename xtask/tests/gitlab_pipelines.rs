use std::{collections::BTreeSet, fs, path::Path};

use serde_yaml_ng::{Mapping, Value};

const MERGE_REQUEST_KIND: &str = "merge-request";

struct GitlabConfig {
    common: Value,
    apple: Value,
}

impl GitlabConfig {
    fn load(root: &Path) -> Self {
        Self {
            common: yaml(root.join(".gitlab/ci/common.yml")),
            apple: yaml(root.join(".gitlab/ci/apple.yml")),
        }
    }

    fn definition(&self, name: &str) -> &Mapping {
        [&self.apple, &self.common]
            .into_iter()
            .find_map(|document| document.as_mapping()?.get(name)?.as_mapping())
            .unwrap_or_else(|| panic!("GitLab configuration has no `{name}` definition"))
    }

    fn extends(&self, name: &str) -> Vec<&str> {
        match self.definition(name).get("extends") {
            None => Vec::new(),
            Some(Value::String(parent)) => vec![parent],
            Some(Value::Sequence(parents)) => parents
                .iter()
                .map(|parent| {
                    parent
                        .as_str()
                        .unwrap_or_else(|| panic!("`{name}` has a non-string parent"))
                })
                .collect(),
            Some(_) => panic!("`{name}` has invalid parents"),
        }
    }

    fn rules_owner(&self, name: &str) -> Option<String> {
        self.rules_owner_inner(name, &mut Vec::new())
    }

    fn rules_owner_inner(&self, name: &str, stack: &mut Vec<String>) -> Option<String> {
        assert!(
            !stack.iter().any(|parent| parent == name),
            "GitLab inheritance cycle through `{name}`"
        );
        stack.push(name.to_owned());

        let mut owner = None;
        for parent in self.extends(name) {
            if let Some(parent_owner) = self.rules_owner_inner(parent, stack) {
                owner = Some(parent_owner);
            }
        }
        if self.definition(name).contains_key("rules") {
            owner = Some(name.to_owned());
        }

        stack.pop();
        owner
    }

    fn effective_value(&self, name: &str, key: &str) -> Option<Value> {
        self.effective_value_inner(name, key, &mut Vec::new())
    }

    fn effective_value_inner(
        &self,
        name: &str,
        key: &str,
        stack: &mut Vec<String>,
    ) -> Option<Value> {
        assert!(
            !stack.iter().any(|parent| parent == name),
            "GitLab inheritance cycle through `{name}`"
        );
        stack.push(name.to_owned());

        let mut value = None;
        for parent in self.extends(name) {
            if let Some(parent_value) = self.effective_value_inner(parent, key, stack) {
                value = Some(parent_value);
            }
        }
        if let Some(own_value) = self.definition(name).get(key) {
            value = Some(own_value.clone());
        }

        stack.pop();
        value
    }

    fn decision_for_kind(&self, rules_owner: &str, kind: &str) -> Option<RuleDecision> {
        let rules = self
            .definition(rules_owner)
            .get("rules")
            .unwrap_or_else(|| panic!("`{rules_owner}` owns no rules"))
            .as_sequence()
            .unwrap_or_else(|| panic!("`{rules_owner}` rules are not a sequence"));

        for rule in rules {
            match rule {
                Value::Mapping(rule) => {
                    for key in rule.keys() {
                        let key = key
                            .as_str()
                            .unwrap_or_else(|| panic!("`{rules_owner}` has a non-string rule key"));
                        assert!(
                            matches!(key, "if" | "when"),
                            "`{rules_owner}` uses unsupported rule key `{key}`"
                        );
                    }
                    let condition = rule
                        .get("if")
                        .unwrap_or_else(|| panic!("`{rules_owner}` has an unconditional rule"));
                    let matches = pipeline_kind(condition, rules_owner) == kind;
                    if matches {
                        return Some(RuleDecision {
                            when: rule.get("when").map(|when| {
                                when.as_str()
                                    .unwrap_or_else(|| {
                                        panic!("`{rules_owner}` has a non-string `when`")
                                    })
                                    .to_owned()
                            }),
                        });
                    }
                }
                Value::Tagged(reference) => {
                    assert!(reference.tag == "reference", "unknown GitLab YAML tag");
                    let target = reference
                        .value
                        .as_sequence()
                        .expect("GitLab reference target is a sequence");
                    assert_eq!(target.len(), 2, "GitLab reference has two components");
                    assert_eq!(target[1].as_str(), Some("rules"));
                    let target = target[0]
                        .as_str()
                        .expect("GitLab reference owner is a string");
                    if let Some(decision) = self.decision_for_kind(target, kind) {
                        return Some(decision);
                    }
                }
                _ => panic!("`{rules_owner}` has an invalid rule"),
            }
        }
        None
    }
}

struct RuleDecision {
    when: Option<String>,
}

fn yaml(path: impl AsRef<Path>) -> Value {
    let path = path.as_ref();
    let text = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_yaml_ng::from_str(&text)
        .unwrap_or_else(|error| panic!("{} is not valid YAML: {error}", path.display()))
}

fn mapping<'a>(value: &'a Value, context: &str) -> &'a Mapping {
    value
        .as_mapping()
        .unwrap_or_else(|| panic!("{context} is not a mapping"))
}

fn pipeline_kind<'a>(condition: &'a Value, owner: &str) -> &'a str {
    condition
        .as_str()
        .and_then(|condition| {
            condition
                .strip_prefix("$KITHARA_PIPELINE_KIND == \"")
                .and_then(|condition| condition.strip_suffix('"'))
        })
        .unwrap_or_else(|| panic!("`{owner}` has an unknown rule condition"))
}

fn assert_active_review_job(config: &GitlabConfig, job: &str, expected_owner: &str) {
    let owner = config
        .rules_owner(job)
        .unwrap_or_else(|| panic!("`{job}` has no effective rules"));
    assert_eq!(owner, expected_owner, "`{job}` uses the wrong rules");

    let decision = config
        .decision_for_kind(&owner, MERGE_REQUEST_KIND)
        .unwrap_or_else(|| panic!("`{job}` does not run for merge requests"));
    assert_automatic_when(decision.when.as_deref(), job);

    let effective_when = config.effective_value(job, "when");
    let effective_when = effective_when.as_ref().map(|when| {
        when.as_str()
            .unwrap_or_else(|| panic!("`{job}` has a non-string effective `when`"))
    });
    assert_automatic_when(effective_when, job);
    assert!(
        config.effective_value(job, "allow_failure").is_none(),
        "`{job}` must remain blocking"
    );
}

fn assert_automatic_when(when: Option<&str>, context: &str) {
    assert!(
        matches!(when, None | Some("on_success")),
        "`{context}` is not an automatic success-gated job"
    );
}

fn assert_exact_rule(rule: &Mapping, condition: &str, when: Option<&str>) {
    let expected_keys = if when.is_some() {
        BTreeSet::from(["if", "when"])
    } else {
        BTreeSet::from(["if"])
    };
    let actual_keys: BTreeSet<&str> = rule
        .keys()
        .map(|key| key.as_str().expect("GitLab rule key is a string"))
        .collect();
    assert_eq!(actual_keys, expected_keys);
    assert_eq!(rule.get("if").and_then(Value::as_str), Some(condition));
    assert_eq!(rule.get("when").and_then(Value::as_str), when);
}

#[test]
fn an_open_merge_request_runs_the_complete_apple_review_matrix() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a workspace root");

    let dispatch = yaml(root.join(".gitlab-ci.yml"));
    let dispatch = mapping(&dispatch, "the dispatch pipeline");
    let workflow = mapping(
        dispatch.get("workflow").expect("dispatch has a workflow"),
        "the dispatch workflow",
    );
    let workflow_rules = workflow
        .get("rules")
        .and_then(Value::as_sequence)
        .expect("dispatch workflow rules are a sequence");
    assert!(workflow_rules.len() >= 4);
    assert_exact_rule(
        mapping(&workflow_rules[0], "the merge-request admission rule"),
        "$CI_PIPELINE_SOURCE == \"merge_request_event\"",
        None,
    );
    assert_exact_rule(
        mapping(&workflow_rules[1], "the default-branch push rule"),
        "$CI_PIPELINE_SOURCE == \"push\" && $CI_COMMIT_BRANCH == $CI_DEFAULT_BRANCH",
        None,
    );
    assert_exact_rule(
        mapping(
            &workflow_rules[2],
            "the open-merge-request suppression rule",
        ),
        "$CI_PIPELINE_SOURCE == \"push\" && $CI_OPEN_MERGE_REQUESTS",
        Some("never"),
    );
    assert_exact_rule(
        mapping(&workflow_rules[3], "the branch push rule"),
        "$CI_PIPELINE_SOURCE == \"push\"",
        None,
    );

    let review = mapping(
        dispatch
            .get("dispatch:merge-request")
            .expect("dispatch has a merge-request job"),
        "the merge-request dispatch",
    );
    assert_eq!(
        review.get("extends").and_then(Value::as_str),
        Some(".serialized")
    );
    let variables = mapping(
        review
            .get("variables")
            .expect("review dispatch has variables"),
        "the review dispatch variables",
    );
    assert_eq!(
        variables
            .get("KITHARA_PIPELINE_KIND")
            .and_then(Value::as_str),
        Some(MERGE_REQUEST_KIND)
    );
    let review_rules = review
        .get("rules")
        .and_then(Value::as_sequence)
        .expect("review dispatch rules are a sequence");
    assert_eq!(review_rules.len(), 1);
    let review_rule = mapping(&review_rules[0], "the review dispatch rule");
    assert_exact_rule(
        review_rule,
        "$CI_PIPELINE_SOURCE == \"merge_request_event\"",
        None,
    );
    assert!(!review.contains_key("trigger"));
    assert_automatic_when(
        review
            .get("when")
            .map(|when| when.as_str().expect("review dispatch `when` is a string")),
        "dispatch:merge-request",
    );
    assert!(!review.contains_key("allow_failure"));

    let serialized = mapping(
        dispatch
            .get(".serialized")
            .expect("dispatch has a serialized template"),
        "the serialized dispatch template",
    );
    assert_automatic_when(
        serialized.get("when").map(|when| {
            when.as_str()
                .expect("serialized dispatch `when` is a string")
        }),
        ".serialized",
    );
    assert!(!serialized.contains_key("allow_failure"));
    let trigger = mapping(
        serialized
            .get("trigger")
            .expect("serialized dispatch has a trigger"),
        "the serialized trigger",
    );
    assert_eq!(
        trigger.get("strategy").and_then(Value::as_str),
        Some("depend")
    );
    let includes = trigger
        .get("include")
        .and_then(Value::as_sequence)
        .expect("serialized trigger includes child configuration");
    assert!(includes.iter().any(|include| {
        include
            .as_mapping()
            .and_then(|include| include.get("local"))
            .and_then(Value::as_str)
            == Some(".gitlab/ci/pipeline.yml")
    }));

    let pipeline = yaml(root.join(".gitlab/ci/pipeline.yml"));
    let pipeline = mapping(&pipeline, "the child pipeline");
    let includes: BTreeSet<&str> = pipeline
        .get("include")
        .and_then(Value::as_sequence)
        .expect("child pipeline includes lane definitions")
        .iter()
        .map(|include| {
            include
                .as_mapping()
                .and_then(|include| include.get("local"))
                .and_then(Value::as_str)
                .expect("child include has a local path")
        })
        .collect();
    assert!(includes.contains(".gitlab/ci/common.yml"));
    assert!(includes.contains(".gitlab/ci/apple.yml"));

    let config = GitlabConfig::load(root);
    let expected_jobs = BTreeSet::from([
        "apple:e2e",
        "apple:ios",
        "apple:ios-test",
        "apple:lint",
        "apple:msrv",
        "apple:safari",
        "apple:swift-test",
        "apple:test",
        "apple:test-flash-off",
        "apple:xcframework",
    ]);
    let actual_jobs: BTreeSet<&str> = mapping(&config.apple, "the Apple pipeline")
        .keys()
        .filter_map(Value::as_str)
        .filter(|name| name.starts_with("apple:"))
        .collect();
    assert_eq!(actual_jobs, expected_jobs);

    for (job, owner) in [
        ("apple:lint", ".rules-verify-and-branch"),
        ("apple:msrv", ".rules-verify"),
        ("apple:test", ".rules-verify-and-branch"),
        ("apple:test-flash-off", ".rules-integration-and-review"),
        ("apple:xcframework", ".rules-verify-and-branch"),
        ("apple:swift-test", ".rules-verify"),
        ("apple:ios", ".rules-verify"),
        ("apple:ios-test", ".rules-integration-and-review"),
        ("apple:e2e", ".rules-review-or-nightly"),
    ] {
        assert_active_review_job(&config, job, owner);
    }

    assert_eq!(
        config.rules_owner("apple:safari").as_deref(),
        Some(".rules-nightly")
    );
    assert!(
        config
            .decision_for_kind(".rules-nightly", MERGE_REQUEST_KIND)
            .is_none()
    );
    let nightly = config
        .decision_for_kind(".rules-nightly", "nightly")
        .expect("Safari runs nightly");
    assert_automatic_when(nightly.when.as_deref(), "apple:safari");
}
