#[macro_use]
macro_rules! cfg_async_std {
    ($($item:item)*) => {
        $(
            #[cfg(feature = "use-async-std")]
            $item
        )*
    }
}

#[macro_use]
macro_rules! cfg_async_std_or_else {
    ($($item:item)*) => {
        $(
            #[cfg(all(feature = "use-tokio", not( feature = "use-async-std" ) ))]
            $item
        )*
    }
}

#[macro_use]
macro_rules! cfg_tokio {
    ($($item:item)*) => {
        $(
            #[cfg(feature = "use-tokio")]
            $item
        )*
    }
}

#[macro_use]
macro_rules! cfg_any_spawner {
    ($($item:item)*) => {
        $(
            #[cfg(any(feature = "use-async-std", feature = "use-tokio"))]
            $item
        )*
    }
}
