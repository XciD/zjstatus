use crate::config::ZellijState;

pub trait Widget {
    fn process(&self, name: &str, state: &ZellijState) -> String;
    fn process_click(&self, name: &str, state: &ZellijState, pos: usize);
    /// Called on timer tick to schedule async work (e.g. command re-execution)
    /// without invalidating widget caches.
    fn tick(&self, _state: &ZellijState) {}
}
