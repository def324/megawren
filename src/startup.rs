/// Keep account setup independent of loading the cloud's file tree.
#[derive(Clone, Copy)]
pub enum Purpose {
    Login,
    UseFiles,
    Logout,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    OfferRemember,
    LoadFiles,
}
pub fn steps(purpose: Purpose, new_session: bool) -> impl Iterator<Item = Step> {
    [
        (new_session && !matches!(purpose, Purpose::Logout)).then_some(Step::OfferRemember),
        matches!(purpose, Purpose::UseFiles).then_some(Step::LoadFiles),
    ]
    .into_iter()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_login_asks_before_saving_without_loading_the_cloud() {
        assert_eq!(
            steps(Purpose::Login, true).collect::<Vec<_>>(),
            [Step::OfferRemember]
        );
    }
    #[test]
    fn first_run_offers_remembering_before_any_cloud_loading() {
        assert_eq!(
            steps(Purpose::UseFiles, true).collect::<Vec<_>>(),
            [Step::OfferRemember, Step::LoadFiles]
        );
    }
    #[test]
    fn restored_login_and_logout_do_not_repeat_setup() {
        assert_eq!(
            steps(Purpose::UseFiles, false).collect::<Vec<_>>(),
            [Step::LoadFiles]
        );
        assert!(steps(Purpose::Logout, false).next().is_none());
    }
}
