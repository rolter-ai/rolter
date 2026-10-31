//! Per-org namespaces for routes, providers and provider groups (#1857,
//! ADR-0040).
//!
//! A route, provider or provider group is named by `(org, name)`: `org` is the
//! row's [`Tenancy::org_id`](crate::Tenancy), and a row with no tenancy, from
//! a gateway-only `rolter.toml`, sits in the global namespace, written `""`.
//! Names are unique per namespace, never across them, so two orgs may both
//! have a `gpt-4o` route or an `openai` provider.
//!
//! A name resolves in the caller's own org first, then globally. A caller with
//! no org (a key from the gateway's own config file, or a row from it) gets the
//! global row, or, with none, the one org row when exactly one org owns the
//! name. A name two orgs own is unknown to it: picking one would be a guess.
//!
//! [`Namespaced`] is that table, shared by the config's validation and
//! sanitizer and by the gateway's snapshot, where every probe is a pair of
//! borrowed `&str` lookups that allocate nothing.

use std::collections::HashMap;

use crate::{GatewayConfig, Tenancy};

/// The global namespace: rows with no tenancy, and keys with no org.
pub const GLOBAL: &str = "";

/// The separator of a qualified runtime id, `name@org_id`.
pub const QUALIFIER: char = '@';

/// The org a row with `tenancy` belongs to, [`GLOBAL`] for none.
pub fn org_of(tenancy: Option<&Tenancy>) -> &str {
    tenancy.map_or(GLOBAL, |t| t.org_id.as_str())
}

/// The runtime id of `name` owned by `org`: `name` itself in the global
/// namespace, `name@org` otherwise.
///
/// The gateway qualifies only a name another row of the same kind also uses
/// (ADR-0040 section 3), so a deployment with no repeated name keeps every
/// key, metric label and log value it had.
pub fn qualify(name: &str, org: &str) -> String {
    if org.is_empty() {
        name.to_string()
    } else {
        format!("{name}{QUALIFIER}{org}")
    }
}

/// The display name of runtime id `id` owned by `org`: the name before the
/// qualifier [`qualify`] added, or `id` itself when it carries none.
pub fn display_name<'a>(id: &'a str, org: &str) -> &'a str {
    if org.is_empty() {
        return id;
    }
    id.strip_suffix(org)
        .and_then(|rest| rest.strip_suffix(QUALIFIER))
        .unwrap_or(id)
}

/// One name's rows: the global one, if any, and one per owning org.
#[derive(Debug, Clone)]
pub struct Scoped<T> {
    global: Option<T>,
    orgs: HashMap<String, T>,
}

impl<T> Default for Scoped<T> {
    fn default() -> Self {
        Self {
            global: None,
            orgs: HashMap::new(),
        }
    }
}

impl<T> Scoped<T> {
    /// The row a caller of `org` resolves, with the org that owns it.
    pub fn resolve(&self, org: &str) -> Option<(&str, &T)> {
        if !org.is_empty() {
            if let Some((owner, value)) = self.orgs.get_key_value(org) {
                return Some((owner.as_str(), value));
            }
        }
        if let Some(value) = &self.global {
            return Some((GLOBAL, value));
        }
        // an org-less caller reaches an org row only when it is unambiguous
        if org.is_empty() && self.orgs.len() == 1 {
            return self
                .orgs
                .iter()
                .next()
                .map(|(owner, value)| (owner.as_str(), value));
        }
        None
    }

    /// The row a **row** of `org` names: its own org's, then the global one,
    /// and nothing else.
    ///
    /// Unlike [`resolve`](Self::resolve), there is no fallback to the one org
    /// that owns the name: a global route or group naming a provider only one
    /// org has would otherwise bind to that org's provider and credential and
    /// serve it to every org's key (ADR-0040 section 1).
    pub fn resolve_ref(&self, org: &str) -> Option<(&str, &T)> {
        if !org.is_empty() {
            if let Some((owner, value)) = self.orgs.get_key_value(org) {
                return Some((owner.as_str(), value));
            }
        }
        self.global.as_ref().map(|value| (GLOBAL, value))
    }

    /// [`resolve`](Self::resolve) for a caller that may not use every row.
    ///
    /// A row of the caller's own org that `admits` refuses (one narrowed to
    /// another project) is no match for that caller, so the lookup goes on to
    /// the global row, which `admits` must pass in its own right (#3546). A
    /// refused global row is a miss.
    pub fn resolve_admitted(&self, org: &str, admits: impl Fn(&T) -> bool) -> Option<(&str, &T)> {
        let (owner, value) = self.resolve(org)?;
        if admits(value) {
            return Some((owner, value));
        }
        if owner == GLOBAL {
            return None;
        }
        self.global
            .as_ref()
            .filter(|value| admits(value))
            .map(|value| (GLOBAL, value))
    }

    /// The row owned by exactly `org`, with no fallback.
    pub fn owned_by(&self, org: &str) -> Option<&T> {
        if org.is_empty() {
            self.global.as_ref()
        } else {
            self.orgs.get(org)
        }
    }

    /// How many rows share the name, across every namespace.
    pub fn len(&self) -> usize {
        usize::from(self.global.is_some()) + self.orgs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every row with its owning org, global first.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &T)> {
        self.global
            .iter()
            .map(|value| (GLOBAL, value))
            .chain(self.orgs.iter().map(|(org, value)| (org.as_str(), value)))
    }
}

/// A table of rows named `(org, name)`.
#[derive(Debug, Clone)]
pub struct Namespaced<T> {
    names: HashMap<String, Scoped<T>>,
}

impl<T> Default for Namespaced<T> {
    fn default() -> Self {
        Self {
            names: HashMap::new(),
        }
    }
}

impl<T> Namespaced<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add `value` as `(org, name)`. A name its namespace already holds is
    /// refused and `value` handed back, so the first row wins on every build.
    pub fn insert(&mut self, org: &str, name: &str, value: T) -> Result<(), T> {
        let scoped = self.names.entry(name.to_string()).or_default();
        let slot = if org.is_empty() {
            &mut scoped.global
        } else {
            match scoped.orgs.entry(org.to_string()) {
                std::collections::hash_map::Entry::Occupied(_) => return Err(value),
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(value);
                    return Ok(());
                }
            }
        };
        if slot.is_some() {
            return Err(value);
        }
        *slot = Some(value);
        Ok(())
    }

    /// The row `name` resolves to for a caller of `org` (see the module docs).
    pub fn get(&self, org: &str, name: &str) -> Option<&T> {
        self.resolve(org, name).map(|(_, value)| value)
    }

    /// [`get`](Self::get), with the org that owns the row.
    pub fn resolve(&self, org: &str, name: &str) -> Option<(&str, &T)> {
        self.names.get(name)?.resolve(org)
    }

    /// [`resolve`](Self::resolve) for a caller that may not use every row:
    /// see [`Scoped::resolve_admitted`].
    pub fn resolve_admitted(
        &self,
        org: &str,
        name: &str,
        admits: impl Fn(&T) -> bool,
    ) -> Option<(&str, &T)> {
        self.names.get(name)?.resolve_admitted(org, admits)
    }

    /// The row a row of `org` names `name` by: see [`Scoped::resolve_ref`].
    /// Route targets, variant targets, group members and semantic-cache
    /// providers resolve this way; keys resolve with [`resolve`](Self::resolve).
    pub fn resolve_ref(&self, org: &str, name: &str) -> Option<(&str, &T)> {
        self.names.get(name)?.resolve_ref(org)
    }

    /// [`resolve_ref`](Self::resolve_ref) without the owner.
    pub fn get_ref(&self, org: &str, name: &str) -> Option<&T> {
        self.resolve_ref(org, name).map(|(_, value)| value)
    }

    /// The row owned by exactly `org`, with no fallback.
    pub fn owned_by(&self, org: &str, name: &str) -> Option<&T> {
        self.names.get(name)?.owned_by(org)
    }

    /// Every row sharing `name`.
    pub fn scoped(&self, name: &str) -> Option<&Scoped<T>> {
        self.names.get(name)
    }

    /// Whether any namespace holds `name`.
    pub fn contains_name(&self, name: &str) -> bool {
        self.names.get(name).is_some_and(|s| !s.is_empty())
    }

    /// Whether more than one namespace holds `name`, which is when the gateway
    /// qualifies its runtime ids.
    pub fn is_shared(&self, name: &str) -> bool {
        self.names.get(name).is_some_and(|s| s.len() > 1)
    }

    /// Every row as `(org, name, value)`.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str, &T)> {
        self.names.iter().flat_map(|(name, scoped)| {
            scoped
                .iter()
                .map(move |(org, value)| (org, name.as_str(), value))
        })
    }

    /// Every row's value.
    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.iter().map(|(_, _, value)| value)
    }

    /// How many rows the table holds, across every name and namespace.
    pub fn len(&self) -> usize {
        self.names.values().map(Scoped::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Leave out every org-owned route, provider and provider group whose name
/// another row of its kind also uses, for a gateway that predates negotiation
/// (ADR-0040 section 5).
///
/// Such a gateway refuses a snapshot with a repeated name and would freeze. Every
/// colliding org row goes, rather than all but one, so no org is preferred and
/// no key can reach another org's row; a global row stays. Runs before the
/// sanitizer, whose cascade then prunes what referenced a provider left out.
/// Returns one problem line per row left out.
pub fn fold_for_legacy_gateway(config: &mut GatewayConfig) -> Vec<String> {
    fn shared<'a>(names: impl Iterator<Item = &'a str>) -> std::collections::HashSet<String> {
        let mut seen = std::collections::HashSet::new();
        let mut shared = std::collections::HashSet::new();
        for name in names {
            if !seen.insert(name) {
                shared.insert(name.to_string());
            }
        }
        shared
    }
    const WHY: &str = "another org uses the same name, and this gateway predates per-org \
                       namespaces; upgrade it to serve both";
    let mut problems = Vec::new();

    let routes = shared(config.routes.iter().map(|r| r.model.as_str()));
    config.routes.retain(|route| {
        let keep = route.tenancy.is_none() || !routes.contains(&route.model);
        if !keep {
            problems.push(format!(
                "route '{}' of org '{}' omitted from the snapshot: {WHY}",
                route.model,
                org_of(route.tenancy.as_ref())
            ));
        }
        keep
    });

    let provider_slug = |p: &crate::ProviderConfig| {
        p.slug
            .clone()
            .unwrap_or_else(|| crate::slug::slugify(&p.name))
    };
    let names = shared(config.providers.iter().map(|p| p.name.as_str()));
    let slugs: Vec<String> = config.providers.iter().map(provider_slug).collect();
    let group_slugs: Vec<String> = config
        .provider_groups
        .iter()
        .map(|g| {
            g.slug
                .clone()
                .unwrap_or_else(|| crate::slug::slugify(&g.name))
        })
        .collect();
    // providers and groups share the `slug/model` address space
    let addresses = shared(slugs.iter().chain(group_slugs.iter()).map(String::as_str));
    let mut left_out = Vec::new();
    let mut index = 0;
    config.providers.retain(|provider| {
        let slug = &slugs[index];
        index += 1;
        let keep = provider.tenancy.is_none()
            || !(names.contains(&provider.name) || addresses.contains(slug));
        if !keep {
            problems.push(format!(
                "provider '{}' of org '{}' omitted from the snapshot: {WHY}",
                provider.name,
                org_of(provider.tenancy.as_ref())
            ));
            left_out.push((
                org_of(provider.tenancy.as_ref()).to_string(),
                provider.name.clone(),
            ));
        }
        keep
    });
    // the sanitizer then reads a reference to one of them as a provider left
    // out, not as a typo, and prunes it
    config.left_out_providers.extend(left_out);
    let mut index = 0;
    config.provider_groups.retain(|group| {
        let slug = &group_slugs[index];
        index += 1;
        let keep = group.tenancy.is_none() || !addresses.contains(slug);
        if !keep {
            problems.push(format!(
                "provider group '{slug}' of org '{}' omitted from the snapshot: {WHY}",
                org_of(group.tenancy.as_ref())
            ));
        }
        keep
    });
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_resolves_in_the_callers_org_first_then_globally() {
        let mut table = Namespaced::new();
        table.insert("", "gpt-4o", "global").unwrap();
        table.insert("org-a", "gpt-4o", "a").unwrap();
        table.insert("org-b", "gpt-4o", "b").unwrap();
        assert_eq!(table.get("org-a", "gpt-4o"), Some(&"a"));
        assert_eq!(table.get("org-b", "gpt-4o"), Some(&"b"));
        // an org with no row of its own falls back to the global one
        assert_eq!(table.get("org-c", "gpt-4o"), Some(&"global"));
        assert_eq!(table.get("", "gpt-4o"), Some(&"global"));
        assert_eq!(table.resolve("org-a", "gpt-4o"), Some(("org-a", &"a")));
        assert_eq!(table.len(), 3);
        assert!(table.is_shared("gpt-4o"));
    }

    #[test]
    fn a_refused_org_row_falls_through_to_the_global_row() {
        let mut table = Namespaced::new();
        table.insert("", "gpt-4o", "global").unwrap();
        table.insert("org-a", "gpt-4o", "a").unwrap();
        let only = |allowed: &'static str| move |value: &&str| *value == allowed;
        // the org's own row while it is usable, the global one once it is not
        assert_eq!(
            table.resolve_admitted("org-a", "gpt-4o", only("a")),
            Some(("org-a", &"a"))
        );
        assert_eq!(
            table.resolve_admitted("org-a", "gpt-4o", only("global")),
            Some(("", &"global"))
        );
        // a refused global row is a miss, and so is a name with no global row
        assert_eq!(table.resolve_admitted("org-b", "gpt-4o", only("a")), None);
        table.insert("org-b", "embed", "b").unwrap();
        assert_eq!(table.resolve_admitted("org-b", "embed", |_| false), None);
    }

    #[test]
    fn an_org_never_resolves_another_orgs_row() {
        let mut table = Namespaced::new();
        table.insert("org-a", "openai", 1).unwrap();
        assert_eq!(table.get("org-b", "openai"), None);
        assert_eq!(table.get("org-a", "openai"), Some(&1));
    }

    #[test]
    fn an_org_less_caller_resolves_an_org_row_only_when_one_org_owns_it() {
        let mut table = Namespaced::new();
        table.insert("org-a", "openai", 1).unwrap();
        assert_eq!(table.get("", "openai"), Some(&1));
        table.insert("org-b", "openai", 2).unwrap();
        assert_eq!(table.get("", "openai"), None, "two owners is a guess");
    }

    #[test]
    fn a_row_never_resolves_an_org_row_it_does_not_belong_to() {
        let mut table = Namespaced::new();
        table.insert("org-a", "openai", 1).unwrap();
        // a caller with no org may reach the sole owner's row; a global row
        // naming it may not, nor may another org's row
        assert_eq!(table.get("", "openai"), Some(&1));
        assert_eq!(table.get_ref("", "openai"), None);
        assert_eq!(table.get_ref("org-b", "openai"), None);
        assert_eq!(table.get_ref("org-a", "openai"), Some(&1));
        table.insert("", "openai", 2).unwrap();
        assert_eq!(table.get_ref("org-b", "openai"), Some(&2));
        assert_eq!(table.get_ref("org-a", "openai"), Some(&1));
    }

    #[test]
    fn a_namespace_holds_a_name_once_and_the_first_row_wins() {
        let mut table = Namespaced::new();
        table.insert("org-a", "openai", 1).unwrap();
        assert_eq!(table.insert("org-a", "openai", 2), Err(2));
        table.insert("", "openai", 3).unwrap();
        assert_eq!(table.insert("", "openai", 4), Err(4));
        assert_eq!(table.owned_by("org-a", "openai"), Some(&1));
        assert_eq!(table.owned_by("", "openai"), Some(&3));
    }

    #[test]
    fn qualify_leaves_a_global_name_alone() {
        assert_eq!(qualify("openai", ""), "openai");
        assert_eq!(qualify("openai", "org-a"), "openai@org-a");
        assert_eq!(display_name("openai@org-a", "org-a"), "openai");
        assert_eq!(display_name("openai", "org-a"), "openai");
        assert_eq!(display_name("openai@org-a", ""), "openai@org-a");
        assert_eq!(display_name("openai@org-b", "org-a"), "openai@org-b");
    }
}
