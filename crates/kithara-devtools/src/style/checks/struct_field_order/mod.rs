mod check;
mod safety;

pub(crate) use check::StructFieldOrder;

#[cfg(test)]
mod contracts;
#[cfg(test)]
mod tests;
