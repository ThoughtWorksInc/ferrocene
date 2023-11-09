// compile-flags: --document-hidden-items

// @has "$.index[*].inner[?(@.import.name=='UsedHidden')]"
// @has "$.index[*][?(@.name=='Hidden')]"
pub mod submodule {
    #[doc(hidden)]
    pub struct Hidden {}
}

pub use submodule::Hidden as UsedHidden;
