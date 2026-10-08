/// Derived widget endpoint qualified by the control's scope suffix.
pub(crate) fn derived(base: &str, scope: &str) -> String {
    let mut endpoint = String::with_capacity(base.len() + scope.len());
    endpoint.push_str(base);
    endpoint.push_str(scope);
    endpoint
}
