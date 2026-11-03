//! Every git call and git file read workuum makes.

mod command;
mod inspect;
mod porcelain;
mod repo;

pub use command::Error;
pub use inspect::{deletable, dirty, last_activity, merged, reachable};
pub use repo::{Repo, resolve};
