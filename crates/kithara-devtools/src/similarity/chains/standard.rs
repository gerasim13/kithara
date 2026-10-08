use super::ty::{ItemPath, Ty};

pub(super) fn prelude(name: &str) -> Option<ItemPath> {
    if matches!(
        name,
        "bool"
            | "char"
            | "str"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "f32"
            | "f64"
    ) {
        return Some(vec![
            "std".to_owned(),
            "primitive".to_owned(),
            name.to_owned(),
        ]);
    }
    let path: &[&str] = match name {
        "Vec" => &["std", "vec", "Vec"],
        "Box" => &["std", "boxed", "Box"],
        "Option" => &["std", "option", "Option"],
        "Some" => &["std", "option", "Option", "Some"],
        "None" => &["std", "option", "Option", "None"],
        "Result" => &["std", "result", "Result"],
        "Ok" => &["std", "result", "Result", "Ok"],
        "Err" => &["std", "result", "Result", "Err"],
        "String" => &["std", "string", "String"],
        "Iterator" => &["std", "iter", "Iterator"],
        "IntoIterator" => &["std", "iter", "IntoIterator"],
        "Drop" => &["std", "ops", "Drop"],
        "Clone" => &["std", "clone", "Clone"],
        "Fn" => &["std", "ops", "Fn"],
        "FnMut" => &["std", "ops", "FnMut"],
        "FnOnce" => &["std", "ops", "FnOnce"],
        _ => return None,
    };
    Some(path.iter().map(|segment| (*segment).to_owned()).collect())
}

pub(super) fn canonical(path: &[String]) -> ItemPath {
    let mut path = path.to_vec();
    if let Some(head) = path.first_mut()
        && matches!(head.as_str(), "core" | "alloc")
    {
        *head = "std".to_owned();
    }
    path
}

fn named(path: &[&str], args: Vec<Vec<Ty>>) -> Ty {
    Ty::named(
        path.iter().map(|segment| (*segment).to_owned()).collect(),
        args,
    )
}

fn option(inner: Vec<Ty>) -> Vec<Ty> {
    vec![named(&["std", "option", "Option"], vec![inner])]
}

fn iterator(inner: Vec<Ty>) -> Vec<Ty> {
    vec![named(&["std", "iter", "Iterator"], vec![inner])]
}

fn is_option(ty: &Ty) -> bool {
    ty.is(&["std", "option", "Option"])
}
fn is_result(ty: &Ty) -> bool {
    ty.is(&["std", "result", "Result"])
}
fn is_vec(ty: &Ty) -> bool {
    ty.is(&["std", "vec", "Vec"])
}
fn is_slice(ty: &Ty) -> bool {
    matches!(ty, Ty::Slice(_))
}

fn is_iterator(ty: &Ty) -> bool {
    ty.is(&["std", "iter", "Iterator"])
        || ty.is(&["std", "iter", "Once"])
        || ty.is(&["std", "iter", "Empty"])
        || ty.is(&["std", "vec", "IntoIter"])
        || ty.is(&["std", "slice", "Iter"])
        || ty.is(&["std", "slice", "IterMut"])
}

pub(super) fn item(ty: &Ty) -> Option<Vec<Ty>> {
    (is_vec(ty) || is_slice(ty) || is_iterator(ty)).then(|| ty.slot(0))
}

pub(super) fn deref(ty: &Ty) -> Option<Vec<Ty>> {
    if ty.is(&["std", "boxed", "Box"])
        || ty.is(&["std", "sync", "Arc"])
        || ty.is(&["std", "rc", "Rc"])
        || ty.is(&["std", "sync", "MutexGuard"])
        || ty.is(&["std", "sync", "RwLockReadGuard"])
        || ty.is(&["std", "sync", "RwLockWriteGuard"])
        || ty.is(&["std", "cell", "Ref"])
        || ty.is(&["std", "cell", "RefMut"])
    {
        Some(ty.slot(0))
    } else if is_vec(ty) {
        Some(vec![Ty::Slice(ty.slot(0))])
    } else {
        None
    }
}

pub(super) fn tried(ty: &Ty) -> Option<Vec<Ty>> {
    (is_option(ty) || is_result(ty)).then(|| ty.slot(0))
}

pub(super) fn indexed(ty: &Ty, scalar: bool) -> Option<Vec<Ty>> {
    (scalar && (is_vec(ty) || is_slice(ty))).then(|| ty.slot(0))
}

pub(super) fn returned(ty: &Ty, method: &str, scalar: bool) -> Option<Vec<Ty>> {
    if (is_option(ty) || is_result(ty)) && matches!(method, "as_ref" | "as_mut") {
        return Some(vec![ty.clone()]);
    }
    if (is_option(ty) || is_result(ty))
        && matches!(method, "unwrap" | "expect" | "unwrap_or_default")
    {
        return Some(ty.slot(0));
    }
    if is_option(ty) && method == "take" {
        return Some(vec![ty.clone()]);
    }
    if (is_vec(ty) || is_slice(ty))
        && (matches!(method, "first" | "first_mut" | "last" | "last_mut")
            || scalar && matches!(method, "get" | "get_mut"))
    {
        return Some(option(ty.slot(0)));
    }
    if (is_vec(ty) || is_slice(ty)) && matches!(method, "iter" | "iter_mut" | "into_iter") {
        return Some(iterator(ty.slot(0)));
    }
    if is_vec(ty) && matches!(method, "as_slice" | "as_mut_slice") {
        return Some(vec![Ty::Slice(ty.slot(0))]);
    }
    if ty.is(&["std", "sync", "Mutex"]) && method == "lock" {
        let guard = named(&["std", "sync", "MutexGuard"], vec![ty.slot(0)]);
        let poison = named(&["std", "sync", "PoisonError"], vec![vec![guard.clone()]]);
        return Some(vec![named(
            &["std", "result", "Result"],
            vec![vec![guard], vec![poison]],
        )]);
    }
    if ty.is(&["std", "collections", "HashMap"]) && method == "entry" {
        return Some(vec![named(
            &["std", "collections", "hash_map", "Entry"],
            vec![ty.slot(0), ty.slot(1)],
        )]);
    }
    if ty.is(&["std", "collections", "HashMap"]) && matches!(method, "get" | "get_mut") {
        return Some(option(ty.slot(1)));
    }
    if ty.is(&["std", "cell", "RefCell"]) && matches!(method, "borrow" | "borrow_mut") {
        let guard = if method == "borrow" { "Ref" } else { "RefMut" };
        return Some(vec![named(&["std", "cell", guard], vec![ty.slot(0)])]);
    }
    if ty.is(&["std", "cell", "OnceCell"]) && matches!(method, "get" | "get_mut") {
        return Some(option(ty.slot(0)));
    }
    if ty.is(&["std", "collections", "hash_map", "OccupiedEntry"])
        && matches!(method, "get" | "get_mut" | "into_mut")
    {
        return Some(ty.slot(1));
    }
    if ty.is(&["std", "collections", "hash_map", "VacantEntry"]) && method == "insert" {
        return Some(ty.slot(1));
    }
    if method == "clone"
        && (is_vec(ty)
            || is_option(ty)
            || is_result(ty)
            || ty.is(&["std", "sync", "Arc"])
            || ty.is(&["std", "rc", "Rc"]))
    {
        return Some(vec![ty.clone()]);
    }
    if method == "as_ref"
        && (ty.is(&["std", "boxed", "Box"])
            || ty.is(&["std", "sync", "Arc"])
            || ty.is(&["std", "rc", "Rc"]))
        || method == "as_mut" && ty.is(&["std", "boxed", "Box"])
        || method == "deref"
        || method == "deref_mut"
            && (is_vec(ty)
                || ty.is(&["std", "boxed", "Box"])
                || ty.is(&["std", "sync", "MutexGuard"])
                || ty.is(&["std", "sync", "RwLockWriteGuard"])
                || ty.is(&["std", "cell", "RefMut"]))
    {
        return deref(ty);
    }
    if is_iterator(ty) {
        match method {
            "next" | "find" => return Some(option(ty.slot(0))),
            "for_each" | "any" | "all" | "map" | "filter_map" | "find_map" => {
                return Some(Vec::new());
            }
            "filter" => return Some(vec![ty.clone()]),
            _ => {}
        }
    }
    None
}

pub(super) fn payload(ty: &Ty, variant: &str, index: &str) -> Option<Vec<Ty>> {
    if index != "0" {
        return None;
    }
    match variant {
        "Some" if is_option(ty) => Some(ty.slot(0)),
        "Ok" if is_result(ty) => Some(ty.slot(0)),
        "Err" if is_result(ty) => Some(ty.slot(1)),
        "Occupied" | "Vacant" if ty.is(&["std", "collections", "hash_map", "Entry"]) => {
            let name = if variant == "Occupied" {
                "OccupiedEntry"
            } else {
                "VacantEntry"
            };
            Some(vec![named(
                &["std", "collections", "hash_map", name],
                vec![ty.slot(0), ty.slot(1)],
            )])
        }
        _ => None,
    }
}

pub(super) fn closure(ty: &Ty, method: &str, arg: usize, input: usize) -> Option<Vec<Ty>> {
    if input == 0
        && arg == 0
        && is_iterator(ty)
        && matches!(
            method,
            "for_each" | "map" | "filter" | "filter_map" | "find" | "find_map" | "any" | "all"
        )
        && let Some(item) = item(ty)
    {
        return Some(item);
    }
    if input == 0
        && ((method == "map_or" || method == "map_or_else") && arg == 1
            || matches!(method, "map" | "and_then") && arg == 0)
        && (is_option(ty) || is_result(ty))
    {
        return Some(ty.slot(0));
    }
    if input == 0 && arg == 0 && matches!(method, "or_else" | "unwrap_or_else") && is_result(ty) {
        return Some(ty.slot(1));
    }
    None
}

pub(super) fn callback_input(ty: &Ty, input: usize) -> Vec<Ty> {
    if !["Fn", "FnMut", "FnOnce"]
        .iter()
        .any(|name| ty.is(&["std", "ops", name]))
    {
        return Vec::new();
    }
    ty.slot(0)
        .iter()
        .filter_map(|args| match args {
            Ty::Tuple(inputs) => inputs.get(input),
            _ => None,
        })
        .flatten()
        .cloned()
        .collect()
}

pub(super) fn constructed(path: &[String], arg: Vec<Ty>) -> Option<Vec<Ty>> {
    let (name, owner) = path.split_last()?;
    let ty = Ty::named(owner.to_vec(), Vec::new());
    if ty.is(&["std", "option", "Option"]) && name == "Some" {
        return Some(option(arg));
    }
    if ty.is(&["std", "result", "Result"]) && matches!(name.as_str(), "Ok" | "Err") {
        let args = if name == "Ok" {
            vec![arg, Vec::new()]
        } else {
            vec![Vec::new(), arg]
        };
        return Some(vec![named(&["std", "result", "Result"], args)]);
    }
    if name == "clone" && (ty.is(&["std", "sync", "Arc"]) || ty.is(&["std", "rc", "Rc"])) {
        return Some(arg);
    }
    if name == "new"
        && (ty.is(&["std", "boxed", "Box"])
            || ty.is(&["std", "sync", "Arc"])
            || ty.is(&["std", "rc", "Rc"]))
    {
        return Some(vec![Ty::named(owner.to_vec(), vec![arg])]);
    }
    None
}
