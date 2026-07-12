#[derive(Debug)]
pub enum ScrollResult {
    TimeLimitExceeded {
        best_difference: f64,
        differences: Vec<f64>,
    },
    EndReached,
    FocusLost,
    Interrupt,
    Success,
    Failed,
    Skip,
}
