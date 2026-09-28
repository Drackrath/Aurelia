//! Steam store genre ids (PICS `common/genres`) to English names.

/// One store genre.
pub struct GenreDef {
    pub id: u32,
    pub name: &'static str,
}

/// Sorted by `id`; validated against the storefront on 2026-09-11.
pub static GENRES: &[GenreDef] = &[
    GenreDef { id: 1, name: "Action" },
    GenreDef { id: 2, name: "Strategy" },
    GenreDef { id: 3, name: "RPG" },
    GenreDef { id: 4, name: "Casual" },
    GenreDef { id: 9, name: "Racing" },
    GenreDef { id: 18, name: "Sports" },
    GenreDef { id: 23, name: "Indie" },
    GenreDef { id: 25, name: "Adventure" },
    GenreDef { id: 28, name: "Simulation" },
    GenreDef { id: 29, name: "Massively Multiplayer" },
    GenreDef { id: 37, name: "Free To Play" },
    GenreDef { id: 50, name: "Accounting" },
    GenreDef { id: 51, name: "Animation & Modeling" },
    GenreDef { id: 52, name: "Audio Production" },
    GenreDef { id: 53, name: "Design & Illustration" },
    GenreDef { id: 54, name: "Education" },
    GenreDef { id: 55, name: "Software Training" },
    GenreDef { id: 56, name: "Utilities" },
    GenreDef { id: 57, name: "Video Production" },
    GenreDef { id: 58, name: "Web Publishing" },
    GenreDef { id: 59, name: "Photo Editing" },
    GenreDef { id: 60, name: "Game Development" },
    GenreDef { id: 70, name: "Early Access" },
    GenreDef { id: 71, name: "Sexual Content" },
    GenreDef { id: 72, name: "Nudity" },
    GenreDef { id: 73, name: "Violent" },
    GenreDef { id: 74, name: "Gore" },
    GenreDef { id: 80, name: "Movie" },
    GenreDef { id: 81, name: "Documentary" },
    GenreDef { id: 82, name: "Episodic" },
    GenreDef { id: 83, name: "Short" },
    GenreDef { id: 84, name: "Tutorial" },
    GenreDef { id: 85, name: "360 Video" },
];

/// English genre name for a PICS genre id.
pub fn genre_name(id: u32) -> Option<&'static str> {
    GENRES
        .binary_search_by_key(&id, |g| g.id)
        .ok()
        .map(|i| GENRES[i].name)
}
