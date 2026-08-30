//! Complete-linkage clustering.
//!
//! This module exists because of a specific, measured failure. Single-linkage
//! (union-find on "any pair close enough") merged 85 photos taken across six different
//! days into one cluster in which only **9% of pairs** were actually similar: A~B and
//! B~C were true, A~C was not, and transitivity did the rest. Of clusters with three or
//! more members, 69 of 115 exceeded the distance threshold in diameter.
//!
//! Complete-linkage — every member within threshold of *every* other member — makes
//! that class of error unrepresentable rather than unlikely. See ADR-0003.

/// Group items so that within a group, `close(i, j)` holds for every pair.
///
/// `pairs` are candidate edges with their distances; anything absent is treated as
/// "not close". Edges are consumed cheapest-first so the tightest groups form before
/// looser members are considered.
pub fn complete_linkage(
    n: usize,
    mut pairs: Vec<(f32, usize, usize)>,
    close: impl Fn(usize, usize) -> bool,
) -> Vec<Vec<usize>> {
    pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut group_of: Vec<Option<usize>> = vec![None; n];
    let mut groups: Vec<Vec<usize>> = Vec::new();

    for (_, i, j) in pairs {
        match (group_of[i], group_of[j]) {
            (None, None) => {
                groups.push(vec![i, j]);
                let g = groups.len() - 1;
                group_of[i] = Some(g);
                group_of[j] = Some(g);
            }
            (Some(g), None) => {
                if groups[g].iter().all(|&m| close(j, m)) {
                    groups[g].push(j);
                    group_of[j] = Some(g);
                }
            }
            (None, Some(g)) => {
                if groups[g].iter().all(|&m| close(i, m)) {
                    groups[g].push(i);
                    group_of[i] = Some(g);
                }
            }
            (Some(a), Some(b)) if a != b => {
                // Merge only if every cross pair is close — never by transitivity.
                let ok = groups[a].iter().all(|&x| groups[b].iter().all(|&y| close(x, y)));
                if ok {
                    let moved = std::mem::take(&mut groups[b]);
                    for &m in &moved {
                        group_of[m] = Some(a);
                    }
                    groups[a].extend(moved);
                }
            }
            _ => {}
        }
    }

    let mut out: Vec<Vec<usize>> = groups.into_iter().filter(|g| g.len() > 1).collect();
    for g in &mut out {
        g.sort_unstable();
    }
    out.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical failure, in miniature: A~B and B~C, but A≁C.
    /// Single-linkage yields one group of three. Complete-linkage must not.
    #[test]
    fn does_not_chain_through_a_middle_member() {
        let close = |i: usize, j: usize| matches!((i.min(j), i.max(j)), (0, 1) | (1, 2));
        let pairs = vec![(0.1, 0, 1), (0.2, 1, 2)];
        let groups = complete_linkage(3, pairs, close);
        assert_eq!(groups.len(), 1, "{groups:?}");
        assert_eq!(groups[0].len(), 2, "chained a non-matching member: {groups:?}");
    }

    /// A genuine clique must still cluster together.
    #[test]
    fn keeps_a_true_clique_whole() {
        let close = |_i: usize, _j: usize| true;
        let pairs = vec![(0.1, 0, 1), (0.1, 1, 2), (0.1, 0, 2)];
        let groups = complete_linkage(3, pairs, close);
        assert_eq!(groups, vec![vec![0, 1, 2]]);
    }

    /// Two separate bursts stay separate.
    #[test]
    fn separates_disjoint_groups() {
        let close = |i: usize, j: usize| (i < 2 && j < 2) || (i >= 2 && j >= 2);
        let pairs = vec![(0.1, 0, 1), (0.1, 2, 3)];
        let mut groups = complete_linkage(4, pairs, close);
        groups.sort();
        assert_eq!(groups, vec![vec![0, 1], vec![2, 3]]);
    }

    /// Every emitted group must be a clique — the defining property.
    #[test]
    fn every_group_is_a_clique() {
        let close = |i: usize, j: usize| (i as i32 - j as i32).abs() <= 1;
        let pairs = vec![(0.1, 0, 1), (0.2, 1, 2), (0.3, 2, 3), (0.4, 0, 3)];
        for g in complete_linkage(4, pairs, close) {
            for &a in &g {
                for &b in &g {
                    assert!(a == b || close(a, b), "group {g:?} is not a clique");
                }
            }
        }
    }
}
