pub mod core_error;
pub mod contract_error;
pub mod wallet_error;
pub mod coin_error;

pub use ate_auth::error::*;

pub use core_error::CoreError;
pub use core_error::CoreErrorKind;
pub use contract_error::ContractError;
pub use contract_error::ContractErrorKind;
pub use wallet_error::WalletError;
pub use wallet_error::WalletErrorKind;
pub use coin_error::CoinError;
pub use coin_error::CoinErrorKind;