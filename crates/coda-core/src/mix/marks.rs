//! A quick mix's marks (decision 8): the genres, artists and albums a
//! frontend's user marked on one source, played together as a mix that is
//! saved nowhere ([`Marks::quick_mix`]). The TUI marks browse rows with a
//! key; each source keeps its own marks.

use super::{Ingredient, MAX_INGREDIENTS, Mix, MixError, MixSource};

/// What [`Marks::toggle`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Toggled {
    /// Marked; this many are marked now.
    Marked(usize),
    /// Unmarked (it was marked); this many are left.
    Unmarked(usize),
    /// Not marked: [`MAX_INGREDIENTS`] are marked already.
    Full,
    /// Not usable as an ingredient (a blank name, say).
    Unusable,
}

/// Ingredients marked on one source, in the order marked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marks {
    source: MixSource,
    marked: Vec<Ingredient>,
}

impl Marks {
    pub fn new(source: MixSource) -> Self {
        Self {
            source,
            marked: Vec::new(),
        }
    }

    pub fn source(&self) -> &MixSource {
        &self.source
    }

    pub fn len(&self) -> usize {
        self.marked.len()
    }

    pub fn is_empty(&self) -> bool {
        self.marked.is_empty()
    }

    pub fn marked(&self) -> &[Ingredient] {
        &self.marked
    }

    /// Whether `ingredient` is marked ([`Ingredient::same_as`]).
    pub fn contains(&self, ingredient: &Ingredient) -> bool {
        let Some(ingredient) = ingredient.clone().cleaned(&self.source) else {
            return false;
        };
        self.marked
            .iter()
            .any(|m| m.same_as(&ingredient, &self.source))
    }

    /// Marks `ingredient`, or unmarks it if it is marked.
    pub fn toggle(&mut self, ingredient: Ingredient) -> Toggled {
        let Some(ingredient) = ingredient.cleaned(&self.source) else {
            return Toggled::Unusable;
        };
        let before = self.marked.len();
        self.marked
            .retain(|m| !m.same_as(&ingredient, &self.source));
        if self.marked.len() < before {
            return Toggled::Unmarked(self.marked.len());
        }
        if self.marked.len() >= MAX_INGREDIENTS {
            return Toggled::Full;
        }
        self.marked.push(ingredient);
        Toggled::Marked(self.marked.len())
    }

    pub fn clear(&mut self) {
        self.marked.clear();
    }

    /// The marks as a quick mix named after them ([`default_name`]). The
    /// marks stay; the caller clears them once it plays.
    pub fn quick_mix(&self) -> Result<Mix, MixError> {
        Mix::new(
            default_name(&self.marked),
            self.source.clone(),
            self.marked.clone(),
        )
    }
}

/// A name for a recipe nobody named (a quick mix): its first names, "Prog
/// Rock + Yes + 2 more" (the GUI's `defaultMixName`).
pub fn default_name(ingredients: &[Ingredient]) -> String {
    if ingredients.is_empty() {
        return "Quick mix".to_owned();
    }
    let shown = if ingredients.len() <= 3 {
        ingredients
    } else {
        &ingredients[..2]
    };
    let mut names: Vec<String> = shown.iter().map(|i| name(i).trim().to_owned()).collect();
    if ingredients.len() > shown.len() {
        names.push(format!("{} more", ingredients.len() - shown.len()));
    }
    names.join(" + ")
}

fn name(ingredient: &Ingredient) -> &str {
    match ingredient {
        Ingredient::Genre { name } | Ingredient::Artist { name, .. } => name,
        Ingredient::Album { name, .. } => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn genre(name: &str) -> Ingredient {
        Ingredient::Genre { name: name.into() }
    }

    fn artist(name: &str, id: Option<&str>) -> Ingredient {
        Ingredient::Artist {
            name: name.into(),
            id: id.map(Into::into),
        }
    }

    #[test]
    fn toggling_marks_and_unmarks_by_the_recipe_rule() {
        let mut marks = Marks::new(MixSource::Local);
        assert_eq!(marks.toggle(genre("Prog Rock")), Toggled::Marked(1));
        assert_eq!(marks.toggle(artist("Yes", None)), Toggled::Marked(2));
        // Local names match ignoring case and surrounding space.
        assert!(marks.contains(&genre(" prog rock")));
        assert_eq!(marks.toggle(genre("PROG ROCK ")), Toggled::Unmarked(1));
        assert!(!marks.contains(&genre("Prog Rock")));
        assert_eq!(marks.marked(), [artist("Yes", None)]);
        assert_eq!(marks.toggle(genre("  ")), Toggled::Unusable);
        marks.clear();
        assert!(marks.is_empty());
    }

    #[test]
    fn navidrome_marks_go_by_id_and_keep_it() {
        let server = crate::navidrome::server_id_of("https://music.example/", "listener");
        let mut marks = Marks::new(MixSource::Navidrome { server });
        assert_eq!(
            marks.toggle(artist("Yes", Some("ar-1"))),
            Toggled::Marked(1)
        );
        // Another artist of the same name is another ingredient.
        assert_eq!(
            marks.toggle(artist("Yes", Some("ar-2"))),
            Toggled::Marked(2)
        );
        assert_eq!(
            marks.toggle(artist("YES", Some("ar-1"))),
            Toggled::Unmarked(1)
        );
        let mix = marks.quick_mix().unwrap();
        assert_eq!(mix.ingredients, [artist("Yes", Some("ar-2"))]);
        assert!(matches!(mix.source, MixSource::Navidrome { .. }));
    }

    #[test]
    fn marks_stop_at_the_ingredient_cap() {
        let mut marks = Marks::new(MixSource::Local);
        for i in 0..MAX_INGREDIENTS {
            assert_eq!(
                marks.toggle(genre(&format!("g{i}"))),
                Toggled::Marked(i + 1)
            );
        }
        assert_eq!(marks.toggle(genre("one more")), Toggled::Full);
        // Unmarking still works at the cap.
        assert_eq!(
            marks.toggle(genre("g0")),
            Toggled::Unmarked(MAX_INGREDIENTS - 1)
        );
    }

    #[test]
    fn a_quick_mix_is_named_after_its_marks() {
        let mut marks = Marks::new(MixSource::Local);
        assert_eq!(marks.quick_mix(), Err(MixError::NoIngredients));
        marks.toggle(genre("Prog Rock"));
        assert_eq!(marks.quick_mix().unwrap().name, "Prog Rock");
        marks.toggle(artist("Yes", None));
        marks.toggle(Ingredient::Album {
            artist: "Genesis".into(),
            name: "Foxtrot".into(),
            id: None,
        });
        assert_eq!(marks.quick_mix().unwrap().name, "Prog Rock + Yes + Foxtrot");
        marks.toggle(artist("King Crimson", None));
        let mix = marks.quick_mix().unwrap();
        assert_eq!(mix.name, "Prog Rock + Yes + 2 more");
        assert_eq!(mix.ingredients.len(), 4);
        // The marks stay until the caller clears them.
        assert_eq!(marks.len(), 4);
        assert_eq!(default_name(&[]), "Quick mix");
    }
}
