#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sidebar {
    Sessions,
    Bookmarks,
    Macros,
    Tools,
    Connections,
    Monitoring,
    Logs,
    Assistant,
}
impl Sidebar {
    pub const ALL: [Self; 8] = [
        Self::Sessions,
        Self::Bookmarks,
        Self::Macros,
        Self::Tools,
        Self::Connections,
        Self::Monitoring,
        Self::Logs,
        Self::Assistant,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Sessions => "Sessions",
            Self::Bookmarks => "Bookmarks",
            Self::Macros => "Macros",
            Self::Tools => "Tools",
            Self::Connections => "Connections",
            Self::Monitoring => "Monitoring",
            Self::Logs => "Logs",
            Self::Assistant => "AI Assistant",
        }
    }
}

/// Assistant/sidebar selection never replaces or retargets the active workspace.
pub struct WorkspaceState<T> {
    pub tabs: Vec<T>,
    pub selected: Option<usize>,
    pub sidebar: Sidebar,
    pub sidebar_visible: bool,
    pub sidebar_width: f32,
}
impl<T> Default for WorkspaceState<T> {
    fn default() -> Self {
        Self {
            tabs: vec![],
            selected: None,
            sidebar: Sidebar::Sessions,
            sidebar_visible: true,
            sidebar_width: 214.0,
        }
    }
}
impl<T> WorkspaceState<T> {
    pub fn relative_tab(&self, delta: isize) -> Option<usize> {
        let count = self.tabs.len();
        if count == 0 {
            return None;
        }
        let current = self.selected.unwrap_or(0).min(count - 1);
        Some(
            (current as isize + delta.rem_euclid(count as isize)).rem_euclid(count as isize)
                as usize,
        )
    }
    pub fn open_new(&mut self, tab: T) {
        self.tabs.push(tab);
        self.selected = Some(self.tabs.len() - 1);
    }
    pub fn select_sidebar(&mut self, section: Sidebar) {
        self.sidebar = section;
        self.sidebar_visible = true;
    }
    pub fn close_tab(&mut self, index: usize) -> Option<T> {
        if index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        self.selected = match self.selected {
            _ if self.tabs.is_empty() => None,
            Some(active) if active > index => Some(active - 1),
            Some(active) if active == index => Some(index.min(self.tabs.len() - 1)),
            value => value,
        };
        Some(tab)
    }
    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        let item = self.tabs.remove(from);
        self.tabs.insert(to, item);
        self.selected = self.selected.map(|active| {
            if active == from {
                to
            } else if from < active && active <= to {
                active - 1
            } else if to <= active && active < from {
                active + 1
            } else {
                active
            }
        });
    }
}

#[derive(Clone, Copy)]
pub struct SidebarDrag {
    start_x: f32,
    start_width: f32,
}
impl SidebarDrag {
    pub fn new(start_x: f32, start_width: f32) -> Self {
        Self {
            start_x,
            start_width,
        }
    }
    pub fn width(self, pointer_x: f32, available_width: f32) -> f32 {
        let max = (available_width - 360.0).clamp(0.0, 900.0);
        let min = 190.0_f32.min(max);
        (self.start_width + pointer_x - self.start_x).clamp(min, max)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selecting_assistant_preserves_tab_and_allows_independent_duplicate_sessions() {
        let mut state = WorkspaceState::default();
        state.open_new("same profile");
        state.open_new("same profile");
        state.select_sidebar(Sidebar::Assistant);
        assert_eq!(state.tabs.len(), 2);
        assert_eq!(state.selected, Some(1));
        state.move_tab(1, 0);
        assert_eq!(state.selected, Some(0));
        state.close_tab(0);
        assert_eq!(state.selected, Some(0));
    }
    #[test]
    fn resize_uses_absolute_pointer_position_even_without_intermediate_frames() {
        let drag = SidebarDrag::new(314.0, 314.0);
        assert_eq!(drag.width(504.0, 1600.0), 504.0);
        assert_eq!(drag.width(454.0, 1600.0), 454.0);
        assert_eq!(drag.width(2000.0, 1600.0), 900.0);
        assert_eq!(drag.width(2000.0, 400.0), 40.0);
    }
    #[test]
    fn navigation_wraps_and_reordering_or_closing_preserves_the_active_identity() {
        let mut state = WorkspaceState::default();
        assert_eq!(state.relative_tab(-1), None);
        for id in ["one", "two", "three", "four"] {
            state.open_new(id);
        }
        assert_eq!(state.relative_tab(1), Some(0));
        state.selected = Some(0);
        assert_eq!(state.relative_tab(-1), Some(3));
        for from in 0..4 {
            for to in 0..4 {
                for active in 0..4 {
                    let mut sample = WorkspaceState::default();
                    for id in ["one", "two", "three", "four"] {
                        sample.open_new(id);
                    }
                    sample.selected = Some(active);
                    let identity = sample.tabs[active];
                    sample.move_tab(from, to);
                    assert_eq!(sample.tabs[sample.selected.unwrap()], identity);
                    let other = (sample.selected.unwrap() + 1) % 4;
                    sample.close_tab(other);
                    assert_eq!(sample.tabs[sample.selected.unwrap()], identity);
                }
            }
        }
        state.close_tab(0);
        assert_eq!(state.tabs[state.selected.unwrap()], "two");
        state.close_tab(0);
        state.close_tab(0);
        state.close_tab(0);
        assert_eq!(state.selected, None);
    }
}
