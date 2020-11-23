# 0.3.3 [2020-04-14]

- Optional support for nom has been added (#27).

# 0.3.2 [2020-03-04]

- Replace the optional `futures` dependency with a `futures` feature that
only includes `futures-io` and `futures-util` as dependencies (#26).

# 0.3.1 [2020-02-17]

- Add modules `io` and `aio` to support direct reading of an unsigned-varint
  value from a `std::io::Read` or `futures::io::AsyncRead` type.

# 0.3.0 [2020-01-02]

- Update to `bytes` v0.5.
- Add support for `tokio-util` v0.2.
- Remove support for `tokio-codec` v0.1.
- Use `#[non_exhaustive]` in `decode::Error` and remove `__Nonexhaustive`.

# 0.2.3 [2019-10-07]

- In addition to `tokio-codec`, `futures_codec` is now supported (#18).
- `decode::Error` now implements `Clone` (#19).
- Code quality improvements (#20, #21).

# 0.2.2 [2019-01-31]

- Add package metadata for docs.rs to generate documentation for all features.

# 0.2.1 [2018-09-05]

- Ensure `codec::Uvi<T>` is `Send` when `T` is.

# 0.2.0 [2018-09-03]

- Change default value for `UviBytes::max` from `usize::MAX` to 128 MiB.

# 0.1.0 [2018-08-08]

Initial release