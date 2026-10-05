/// The keys of a toolkit's physical key code `$code` that bear a Latin letter,
/// each beside its letter. Every decoder names shortcut keys by this one table,
/// so a shortcut is the same key whichever toolkit read it.
#[macro_export]
macro_rules! letters {
    ($code:ident) => {
        [
            ($code::KeyA, 'a'),
            ($code::KeyB, 'b'),
            ($code::KeyC, 'c'),
            ($code::KeyD, 'd'),
            ($code::KeyE, 'e'),
            ($code::KeyF, 'f'),
            ($code::KeyG, 'g'),
            ($code::KeyH, 'h'),
            ($code::KeyI, 'i'),
            ($code::KeyJ, 'j'),
            ($code::KeyK, 'k'),
            ($code::KeyL, 'l'),
            ($code::KeyM, 'm'),
            ($code::KeyN, 'n'),
            ($code::KeyO, 'o'),
            ($code::KeyP, 'p'),
            ($code::KeyQ, 'q'),
            ($code::KeyR, 'r'),
            ($code::KeyS, 's'),
            ($code::KeyT, 't'),
            ($code::KeyU, 'u'),
            ($code::KeyV, 'v'),
            ($code::KeyW, 'w'),
            ($code::KeyX, 'x'),
            ($code::KeyY, 'y'),
            ($code::KeyZ, 'z'),
        ]
    };
}
