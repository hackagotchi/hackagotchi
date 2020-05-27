use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fmt;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone)]
pub enum ConfigError {
    UnknownArchetypeName(String),
}
impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ConfigError::*;
        match self {
            UnknownArchetypeName(name) => write!(f, "no archetype by the name of {:?}", name),
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub special_users: Vec<String>,
    pub profile_archetype: ProfileArchetype,
    pub plant_archetypes: Vec<PlantArchetype>,
    pub possession_archetypes: Vec<Archetype>,
}
impl Config {
    #[allow(dead_code)]
    pub fn find_plant<S: AsRef<str>>(&self, name: &S) -> Result<&PlantArchetype, ConfigError> {
        self.plant_archetypes
            .iter()
            .find(|x| name.as_ref() == x.name)
            .ok_or(ConfigError::UnknownArchetypeName(name.as_ref().to_string()))
    }
    pub fn find_plant_handle<S: AsRef<str>>(
        &self,
        name: &S,
    ) -> Result<ArchetypeHandle, ConfigError> {
        self.plant_archetypes
            .iter()
            .position(|x| name.as_ref() == x.name)
            .ok_or(ConfigError::UnknownArchetypeName(name.as_ref().to_string()))
    }
    #[allow(dead_code)]
    pub fn find_possession<S: AsRef<str>>(&self, name: &S) -> Result<&Archetype, ConfigError> {
        self.possession_archetypes
            .iter()
            .find(|x| name.as_ref() == x.name)
            .ok_or(ConfigError::UnknownArchetypeName(name.as_ref().to_string()))
    }
    pub fn find_possession_handle<S: AsRef<str>>(
        &self,
        name: &S,
    ) -> Result<ArchetypeHandle, ConfigError> {
        self.possession_archetypes
            .iter()
            .position(|x| name.as_ref() == x.name)
            .ok_or(ConfigError::UnknownArchetypeName(name.as_ref().to_string()))
    }
}

// I should _really_ use a different version of this for PlantArchetypes and PossessionArchetypes ...
pub type ArchetypeHandle = usize;

lazy_static::lazy_static! {
    pub static ref CONFIG: Config = {
        pub fn f<T: DeserializeOwned>(p: &'static str) -> T {
            serde_json::from_str(
                &std::fs::read_to_string(format!(
                    concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/config/{}.json",
                    ),
                    p
                ))
                .unwrap_or_else(|e| panic!("opening {}: {}", p, e))
            )
            .unwrap_or_else(|e| panic!("parsing {}: {}", p, e))
        }

        Config {
            special_users: f("special_users"),
            profile_archetype: ProfileArchetype {
                advancements: f("hackstead_advancements"),
            },
            plant_archetypes: f("plant_archetypes"),
            possession_archetypes: f("possession_archetypes"),
        }
    };
}

#[derive(Deserialize, Debug, Clone)]
pub struct ProfileArchetype {
    pub advancements: AdvancementSet<HacksteadAdvancementSum>,
}

pub type HacksteadAdvancement = Advancement<HacksteadAdvancementSum>;
#[derive(Deserialize, Debug, Clone, PartialEq)]
pub enum HacksteadAdvancementKind {
    Land { pieces: u32 },
}
#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct HacksteadAdvancementSum {
    pub land: u32,
    pub xp: u64,
}
impl AdvancementSum for HacksteadAdvancementSum {
    type Kind = HacksteadAdvancementKind;

    fn new(unlocked: &[&Advancement<Self>]) -> Self {
        Self {
            xp: unlocked.iter().fold(0, |a, c| a + c.xp),
            land: unlocked
                .iter()
                .map(|k| match k.kind {
                    HacksteadAdvancementKind::Land { pieces } => pieces,
                })
                .sum(),
        }
    }

    fn filter_base(_a: &Advancement<Self>) -> bool {
        true
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct GotchiArchetype {
    pub base_happiness: u64,
}
#[derive(Deserialize, Debug, Clone)]
pub struct SeedArchetype {
    pub grows_into: String,
}
#[derive(Deserialize, Debug, Clone)]
pub enum ApplicationEffect {
    TimeIncrease {
        extra_cycles: u64,
        duration_cycles: u64,
    },
}
#[derive(Deserialize, Debug, Clone)]
pub struct LandUnlock {
    pub requires_xp: bool,
}
#[derive(Deserialize, Debug, Clone)]
pub struct KeepsakeArchetype {
    pub item_application_effect: Option<ApplicationEffect>,
    pub unlocks_land: Option<LandUnlock>,
}

#[derive(Deserialize, Debug, Clone)]
pub enum ArchetypeKind {
    Gotchi(GotchiArchetype),
    Seed(SeedArchetype),
    Keepsake(KeepsakeArchetype),
}
impl ArchetypeKind {
    pub fn category(&self) -> crate::Category {
        use crate::Category;
        match self {
            ArchetypeKind::Gotchi(_) => Category::Gotchi,
            _ => Category::Misc,
        }
    }
    pub fn keepsake(&self) -> Option<&KeepsakeArchetype> {
        match self {
            ArchetypeKind::Keepsake(k) => Some(k),
            _ => None,
        }
    }
}
#[derive(Deserialize, Debug, Clone)]
pub struct Archetype {
    pub name: String,
    pub description: String,
    pub kind: ArchetypeKind,
}

#[derive(Deserialize, Debug, Clone)]
pub struct PlantArchetype {
    pub name: String,
    pub base_yield_duration: f32,
    pub advancements: AdvancementSet<PlantAdvancementSum>,
}
impl Eq for PlantArchetype {}
impl PartialEq for PlantArchetype {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}
impl Hash for PlantArchetype {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}
pub type PlantAdvancement = Advancement<PlantAdvancementSum>;
/// Recipe is generic over the way Archetypes are referred to
/// to make it easy to use Strings in the configs and ArchetypeHandles
/// at runtime
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Recipe<Handle> {
    pub needs: Vec<(usize, Handle)>,
    pub makes: Handle,
    pub destroys_plant: bool,
    pub time: f32,
}
impl Recipe<ArchetypeHandle> {
    pub fn satisfies(&self, inv: &[crate::Possession]) -> bool {
        self.needs.iter().copied().all(|(count, ah)| {
            let has = inv.iter().filter(|x| x.archetype_handle == ah).count();
            count <= has
        })
    }
    pub fn lookup_handles(self) -> Option<Recipe<&'static Archetype>> {
        let makes = CONFIG.possession_archetypes.get(self.makes)?;
        let needs = self
            .needs
            .into_iter()
            .map(|(n, x)| Some((n, CONFIG.possession_archetypes.get(x)?)))
            .collect::<Option<Vec<(_, &Archetype)>>>()?;

        Some(Recipe {
            makes,
            needs,
            time: self.time,
            destroys_plant: self.destroys_plant,
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct SpawnRate(pub f32, pub (f32, f32));
impl SpawnRate {
    pub fn gen_count<R: rand::Rng>(self, rng: &mut R) -> usize {
        let Self(guard, (lo, hi)) = self;
        if rng.gen_range(0.0, 1.0) < guard {
            let chance = rng.gen_range(lo, hi);
            let base = chance.floor();
            let extra = if rng.gen_range(0.0, 1.0) < chance - base {
                1
            } else {
                0
            };
            base as usize + extra
        } else {
            0
        }
    }
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
pub enum PlantAdvancementKind {
    Xp(f32),
    YieldSpeed(f32),
    YieldSize(f32),
    Neighbor(Box<PlantAdvancementKind>),
    Yield(Vec<(SpawnRate, String)>),
    Craft(Vec<Recipe<String>>),
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(bound(deserialize = ""))]
pub struct PlantAdvancementSum {
    pub xp: u64,
    pub xp_multiplier: f32,
    pub yield_speed_multiplier: f32,
    pub yield_size_multiplier: f32,
    pub yields: Vec<(SpawnRate, ArchetypeHandle)>,
    pub recipes: Vec<Recipe<ArchetypeHandle>>,
}
impl AdvancementSum for PlantAdvancementSum {
    type Kind = PlantAdvancementKind;

    fn new(unlocked: &[&Advancement<Self>]) -> Self {
        use PlantAdvancementKind::*;

        let mut xp = 0;
        let mut xp_multiplier = 1.0;
        let mut yield_speed_multiplier = 1.0;
        let mut yield_size_multiplier = 1.0;
        let mut yields = vec![];
        let mut recipes = vec![];

        for k in unlocked.iter() {
            xp += k.xp;

            // apply neighbor upgrades as if they weren't neighbor upgrades :D
            let kind = match &k.kind {
                Neighbor(n) => &**n,
                other => other,
            };

            match kind {
                Xp(multiplier) => xp_multiplier *= multiplier,
                YieldSpeed(multiplier) => yield_speed_multiplier *= multiplier,
                Neighbor(..) => {}
                YieldSize(multiplier) => yield_size_multiplier *= multiplier,
                Yield(resources) => yields.append(
                    &mut resources
                        .iter()
                        .map(|(c, s)| Ok((*c, CONFIG.find_possession_handle(s)?)))
                        .collect::<Result<Vec<_>, ConfigError>>()
                        .expect("couldn't find archetype for advancement yield"),
                ),
                Craft(new_recipes) => recipes.append(
                    &mut new_recipes
                        .iter()
                        .map(|r| {
                            Ok(Recipe {
                                makes: CONFIG.find_possession_handle(&r.makes)?,
                                needs: r
                                    .needs
                                    .iter()
                                    .map(|(c, s)| Ok((*c, CONFIG.find_possession_handle(s)?)))
                                    .collect::<Result<Vec<_>, ConfigError>>()?,

                                time: r.time,
                                destroys_plant: r.destroys_plant,
                            })
                        })
                        .collect::<Result<Vec<_>, ConfigError>>()
                        .expect("couldn't find archetype for crafting advancement"),
                ),
            }
        }

        yields = yields
            .into_iter()
            .map(|(SpawnRate(guard, (lo, hi)), name)| {
                (
                    SpawnRate(
                        (guard * yield_size_multiplier).min(1.0),
                        (lo * yield_size_multiplier, hi * yield_size_multiplier),
                    ),
                    name,
                )
            })
            .collect();

        Self {
            xp,
            xp_multiplier,
            yield_speed_multiplier,
            yield_size_multiplier,
            yields,
            recipes,
        }
    }

    // ignore your neighbor bonuses you give out
    fn filter_base(a: &Advancement<Self>) -> bool {
        match &a.kind {
            PlantAdvancementKind::Neighbor(..) => false,
            _ => true,
        }
    }
}

pub trait AdvancementSum: DeserializeOwned + PartialEq + fmt::Debug {
    type Kind: DeserializeOwned + fmt::Debug + Clone + PartialEq;

    fn new(unlocked: &[&Advancement<Self>]) -> Self;
    fn filter_base(a: &Advancement<Self>) -> bool;
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(bound(deserialize = ""))]
pub struct Advancement<S: AdvancementSum> {
    pub kind: S::Kind,
    pub xp: u64,
    pub art: String,
    pub title: String,
    pub description: String,
    pub achiever_title: String,
}
#[derive(Deserialize, Debug, Clone)]
#[serde(bound(deserialize = ""))]
pub struct AdvancementSet<S: AdvancementSum> {
    pub base: Advancement<S>,
    rest: Vec<Advancement<S>>,
}
#[allow(dead_code)]
impl<S: AdvancementSum> AdvancementSet<S> {
    pub fn all(&self) -> impl Iterator<Item = &Advancement<S>> {
        std::iter::once(&self.base).chain(self.rest.iter())
    }
    pub fn unlocked(&self, xp: u64) -> impl Iterator<Item = &Advancement<S>> {
        std::iter::once(&self.base).chain(self.rest.iter().take(self.current_position(xp)))
    }

    pub fn get(&self, index: usize) -> Option<&Advancement<S>> {
        if index == 0 {
            Some(&self.base)
        } else {
            self.rest.get(index - 1)
        }
    }

    pub fn increment_xp(&self, xp: &mut u64) -> Option<&Advancement<S>> {
        *xp += 1;
        self.next(*xp - 1)
            .filter(|&x| self.next(*xp).map(|n| *x != *n).unwrap_or(false))
    }

    pub fn sum<'a>(
        &'a self,
        xp: u64,
        extra_advancements: impl Iterator<Item = &'a Advancement<S>>,
    ) -> S {
        S::new(
            &self
                .unlocked(xp)
                .filter(|&x| S::filter_base(x))
                .chain(extra_advancements)
                .collect::<Vec<_>>(),
        )
    }

    pub fn raw_sum(&self, xp: u64) -> S {
        S::new(&self.unlocked(xp).collect::<Vec<_>>())
    }

    pub fn max<'a>(&'a self, extra_advancements: impl Iterator<Item = &'a Advancement<S>>) -> S {
        S::new(
            &self
                .all()
                .filter(|&x| S::filter_base(x))
                .chain(extra_advancements)
                .collect::<Vec<_>>(),
        )
    }

    pub fn current(&self, xp: u64) -> &Advancement<S> {
        self.get(self.current_position(xp)).unwrap_or(&self.base)
    }

    pub fn next(&self, xp: u64) -> Option<&Advancement<S>> {
        self.get(self.current_position(xp) + 1)
    }

    pub fn current_position(&self, xp: u64) -> usize {
        let mut state = 0;
        self.all()
            .position(|x| {
                state += x.xp;
                state > xp
            })
            .unwrap_or(self.rest.len() + 1)
            .checked_sub(1)
            .unwrap_or(0)
    }
}

#[test]
fn upgrade_increase() {
    for arch in CONFIG.plant_archetypes.iter() {
        let adv = &arch.advancements;
        let last = adv.rest.last().unwrap();
        for xp in 0..last.xp {
            assert!(
                adv.current(xp).xp <= xp,
                "when xp is {} for {} the current advancement has more xp({})",
                xp,
                arch.name,
                adv.current(xp).xp
            );
        }
    }
}

#[test]
/// In the CONFIG, you can specify the names of archetypes.
/// If you're Rishi, you might spell one of those names wrong.
/// This test helps you make sure you didn't do that.
fn archetype_name_matches() {
    for a in CONFIG.possession_archetypes.iter() {
        match &a.kind {
            ArchetypeKind::Seed(sa) => assert!(
                CONFIG.find_plant(&sa.grows_into).is_ok(),
                "seed archetype {:?} claims it grows into unknown plant archetype {:?}",
                a.name,
                sa.grows_into,
            ),
            _ => {}
        }
    }

    for arch in CONFIG.plant_archetypes.iter().cloned() {
        for adv in arch.advancements.all() {
            use PlantAdvancementKind::*;

            match &adv.kind {
                Yield(resources) => {
                    for (_, item_name) in resources.iter() {
                        assert!(
                            CONFIG.find_possession(item_name).is_ok(),
                            "Yield advancement {:?} for plant {:?} includes unknown resource {:?}",
                            adv.title,
                            arch.name,
                            item_name,
                        )
                    }
                }
                Craft(recipes) => {
                    for Recipe { makes, needs, .. } in recipes.iter() {
                        assert!(
                            CONFIG.find_possession(makes).is_ok(),
                            "Crafting advancement {:?} for plant {:?} produces unknown resource {:?}",
                            adv.title,
                            arch.name,
                            makes,
                        );
                        for (_, resource) in needs.iter() {
                            assert!(
                                CONFIG.find_possession(resource).is_ok(),
                                "Crafting advancement {:?} for plant {:?} uses unknown resource {:?} in recipe for {:?}",
                                adv.title,
                                arch.name,
                                resource,
                                makes
                            )
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
