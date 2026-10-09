//! The viewport overrides sessions have set on one page.
//!
//! The page's own viewport is kept as it was before the first override, each
//! session owns at most one entry, and the newest entry is the one in force.
//! A session that leaves removes its entry: the next newest takes over, and
//! the last one out puts the pristine viewport back, whatever order they
//! leave in.

use tenta_browser::Viewport;

#[derive(Default)]
pub(crate) struct ViewportOverrides {
    pristine: Option<Viewport>,
    stack: Vec<(usize, Viewport)>,
}

impl ViewportOverrides {
    /// The viewport before any override; `now` when there is none.
    pub(crate) fn pristine_or(&self, now: Viewport) -> Viewport {
        self.pristine.unwrap_or(now)
    }

    /// Records `owner`'s override, which becomes the one in force.
    pub(crate) fn set(&mut self, owner: usize, now: Viewport, viewport: Viewport) {
        if self.stack.is_empty() {
            self.pristine = Some(now);
        }
        self.stack.retain(|(known, _)| *known != owner);
        self.stack.push((owner, viewport));
    }

    /// Drops `owner`'s override. The viewport to apply, when it changes what
    /// is in force: the newest remaining override, or the pristine one.
    pub(crate) fn clear(&mut self, owner: usize) -> Option<Viewport> {
        let before = self.stack.len();
        self.stack.retain(|(known, _)| *known != owner);
        if self.stack.len() == before {
            return None;
        }
        match self.stack.last() {
            Some((_, viewport)) => Some(*viewport),
            None => self.pristine.take(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(width: u32) -> Viewport {
        Viewport {
            width,
            ..Viewport::default()
        }
    }

    #[test]
    fn the_last_session_out_restores_the_pristine_viewport_in_any_order() {
        for order in [[1, 2], [2, 1]] {
            let mut stack = ViewportOverrides::default();
            stack.set(1, at(1280), at(300));
            stack.set(2, at(300), at(500));
            assert_eq!(stack.pristine_or(at(0)), at(1280));
            let first = stack.clear(order[0]);
            let last = stack.clear(order[1]);
            assert_eq!(last, Some(at(1280)), "order {order:?}");
            // Leaving while the other still holds the newest keeps it.
            let expected = if order[0] == 1 { at(500) } else { at(300) };
            assert_eq!(first, Some(expected), "order {order:?}");
        }
    }

    #[test]
    fn clearing_without_an_override_changes_nothing() {
        let mut stack = ViewportOverrides::default();
        assert_eq!(stack.clear(7), None);
    }
}
