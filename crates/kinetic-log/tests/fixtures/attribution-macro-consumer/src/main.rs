use kinetic_log_attribution_macro_support::FixtureAttributable;

fn main() {
    let _span = kinetic_log::attribution_span!(&FixtureAttributable);
}
