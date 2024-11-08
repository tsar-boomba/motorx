#[macro_export]
/// Everything in this block is only compiled with logging feature
macro_rules! cfg_logging {
	($($item:item)*) => {
        #[cfg(feature = "logging")]
        {
            $(
                $item
            )*
        };
    }
}
