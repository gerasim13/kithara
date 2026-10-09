mod analysis;
pub(crate) mod consts;

pub(crate) use analysis::RedundantAccessors;

#[cfg(test)]
mod tests;
