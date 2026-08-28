//! Deciding who a face belongs to.
//!
//! The rule is **discriminative**, not a bare threshold, and that distinction is the
//! whole point of this module. During the manual session a plain "similarity >= 0.40"
//! test pulled a different man and a blonde woman into someone's folder. Requiring the
//! best-matching person to also beat the runner-up by a margin removes that class of
//! error: a stranger tends to score mediocre-but-similar against everyone, so nobody
//! wins clearly. See ADR-0003.

use crate::faces::{embed::cosine, people::People};

/// Minimum similarity to assign at all. Below this, leave the photo alone.
pub const DEFAULT_MIN_SIM: f32 = 0.50;
/// How far the best match must beat the runner-up.
pub const DEFAULT_MARGIN: f32 = 0.05;

#[derive(Debug, Clone, PartialEq)]
pub enum Assignment {
    /// Confidently this person.
    Person { name: String, similarity: f32, runner_up: Option<(String, f32)> },
    /// Someone is recognisable but two identities are too close to separate.
    Ambiguous { best: String, best_sim: f32, second: String, second_sim: f32 },
    /// Nothing matched well enough. Never moved; always reported.
    Unknown { best_sim: f32 },
}

impl Assignment {
    pub fn person(&self) -> Option<&str> {
        match self {
            Assignment::Person { name, .. } => Some(name),
            _ => None,
        }
    }
}

pub struct Options {
    pub min_sim: f32,
    pub margin: f32,
}

impl Default for Options {
    fn default() -> Self {
        Self { min_sim: DEFAULT_MIN_SIM, margin: DEFAULT_MARGIN }
    }
}

/// Score a face against every known person. A person's score is the similarity to
/// their *closest* reference, not to an average — see `Person::references`.
pub fn score_all(embedding: &[f32], people: &People) -> Vec<(String, f32)> {
    let mut v: Vec<(String, f32)> = people
        .people
        .iter()
        .map(|p| {
            let best = p
                .references
                .iter()
                .map(|r| cosine(embedding, r))
                .fold(f32::NEG_INFINITY, f32::max);
            (p.name.clone(), if best.is_finite() { best } else { -1.0 })
        })
        .collect();
    v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    v
}

pub fn assign(embedding: &[f32], people: &People, opt: &Options) -> Assignment {
    let scores = score_all(embedding, people);
    let Some((best_name, best)) = scores.first().cloned() else {
        return Assignment::Unknown { best_sim: 0.0 };
    };
    if best < opt.min_sim {
        return Assignment::Unknown { best_sim: best };
    }
    match scores.get(1).cloned() {
        Some((second_name, second)) if best - second < opt.margin => Assignment::Ambiguous {
            best: best_name,
            best_sim: best,
            second: second_name,
            second_sim: second,
        },
        other => Assignment::Person {
            name: best_name,
            similarity: best,
            runner_up: other,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::faces::people::Person;

    fn unit(v: Vec<f32>) -> Vec<f32> {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        v.iter().map(|x| x / n).collect()
    }

    fn people_with(pairs: &[(&str, Vec<Vec<f32>>)]) -> People {
        People {
            people: pairs
                .iter()
                .map(|(n, r)| Person { name: n.to_string(), references: r.clone(), excluded: Vec::new() })
                .collect(),
        }
    }

    #[test]
    fn assigns_a_clear_match() {
        let me = unit(vec![1.0, 0.0, 0.0]);
        let other = unit(vec![0.0, 1.0, 0.0]);
        let p = people_with(&[("Me", vec![me.clone()]), ("Nikhil", vec![other])]);
        let a = assign(&me, &p, &Options::default());
        assert_eq!(a.person(), Some("Me"));
    }

    /// A stranger scores middling against everyone. A bare threshold at 0.40 would
    /// have accepted this; the margin rule must not.
    #[test]
    fn refuses_a_stranger_who_is_vaguely_similar_to_two_people() {
        let a = unit(vec![1.0, 0.0, 0.0]);
        let b = unit(vec![0.0, 1.0, 0.0]);
        let stranger = unit(vec![1.0, 1.0, 0.0]); // ~0.707 to both, equally
        let p = people_with(&[("A", vec![a]), ("B", vec![b])]);
        let out = assign(&stranger, &p, &Options::default());
        assert!(
            matches!(out, Assignment::Ambiguous { .. }),
            "a stranger equidistant from two people must not be assigned: {out:?}"
        );
    }

    #[test]
    fn refuses_below_the_floor() {
        let a = unit(vec![1.0, 0.0, 0.0]);
        let far = unit(vec![0.1, 1.0, 0.0]);
        let p = people_with(&[("A", vec![a])]);
        assert!(matches!(assign(&far, &p, &Options::default()), Assignment::Unknown { .. }));
    }

    /// The same person in profile embeds differently from front-on. Keeping every
    /// reference rather than a centroid is what lets both match.
    #[test]
    fn matches_a_second_pose_via_its_own_reference() {
        let front = unit(vec![1.0, 0.0, 0.0]);
        let profile = unit(vec![0.0, 0.0, 1.0]);
        let other = unit(vec![0.0, 1.0, 0.0]);
        let p = people_with(&[
            ("Me", vec![front.clone(), profile.clone()]),
            ("Other", vec![other]),
        ]);
        assert_eq!(assign(&profile, &p, &Options::default()).person(), Some("Me"));
        assert_eq!(assign(&front, &p, &Options::default()).person(), Some("Me"));
    }

    #[test]
    fn no_people_means_unknown() {
        assert!(matches!(
            assign(&unit(vec![1.0, 0.0, 0.0]), &People::default(), &Options::default()),
            Assignment::Unknown { .. }
        ));
    }
}
