pub use std::fmt::Debug;

use serde::{Deserialize, Serialize};
use zbus::zvariant;

pub trait Builtin: Debug + Send + Sync + 'static {
    fn config(&self) -> &'static str;
    fn name(&self) -> &'static str;
}
