# Changelog

## [0.3.1](https://github.com/SatoshiAndKin/create2-salt-miner/compare/salty-v0.3.0...salty-v0.3.1) (2026-09-10)


### Bug Fixes

* enforce mining limits across local and remote jobs ([8fced24](https://github.com/SatoshiAndKin/create2-salt-miner/commit/8fced24df9928eba20545e00272fdf3b0280e518))
* honor mining limits, validate PGO, and speed up GPU mining ([549fd17](https://github.com/SatoshiAndKin/create2-salt-miner/commit/549fd177fc2041e89f49fdc41d61e45c2f2e6233))
* migrate stored request keys and honor Cargo target paths ([71e7669](https://github.com/SatoshiAndKin/create2-salt-miner/commit/71e7669b02cb2d51ccf5453238045fb7179db779))
* preserve stored requests and use configured Windows build paths ([17010d9](https://github.com/SatoshiAndKin/create2-salt-miner/commit/17010d951d3d078f3219d201d23dc0e4096acaf4))


### Performance Improvements

* make kernel optimizations work across GPU compilers ([76f24fe](https://github.com/SatoshiAndKin/create2-salt-miner/commit/76f24fe8763870d9ea5e9bbf84311f307af15ab7))
* partially unroll Keccak rounds after paired Metal trials ([6a02e93](https://github.com/SatoshiAndKin/create2-salt-miner/commit/6a02e935344bee2899771ff3ee9f037f5af0682d))
* use paired 32-bit Keccak rotations after Metal confirmation ([3055751](https://github.com/SatoshiAndKin/create2-salt-miner/commit/305575174838b3008345226540169d2090de2f2a))

## [0.3.0](https://github.com/SatoshiAndKin/create2-salt-miner/compare/salty-v0.2.0...salty-v0.3.0) (2026-08-09)


### Features

* add cargo-pgo and gated BOLT build targets ([cda5fe9](https://github.com/SatoshiAndKin/create2-salt-miner/commit/cda5fe9b8eb412b046ef97ee0445f72152d49c00))
* add max-runtime-secs and rename once to one ([36705db](https://github.com/SatoshiAndKin/create2-salt-miner/commit/36705db57b908a6362a8fb0ce99e5a5cb994da19))
* add minimum-runtime gated best-result mining exits ([c20e133](https://github.com/SatoshiAndKin/create2-salt-miner/commit/c20e1338dc3b644e90fb1ba3e21742f6ec248aba))
* add remote server mining config ([2b3bbfe](https://github.com/SatoshiAndKin/create2-salt-miner/commit/2b3bbfe6570d36eaed69e4c4188416f3ad34608a))
* improve mining controls and optimized releases ([4c8f186](https://github.com/SatoshiAndKin/create2-salt-miner/commit/4c8f1862fb966259ac3a2953c73322140afd0c5e))


### Bug Fixes

* allow bench defaults without config ([29f1b4e](https://github.com/SatoshiAndKin/create2-salt-miner/commit/29f1b4ed482169a04979b8ebe049a77e16065151))
* use Metal mining on macOS ([bdbea28](https://github.com/SatoshiAndKin/create2-salt-miner/commit/bdbea2835e69c470e568b002a35f21c49ffb80c7))

## [0.2.0](https://github.com/SatoshiAndKin/create2-salt-miner/compare/salty-v0.1.0...salty-v0.2.0) (2026-05-16)


### Features

* add just windows for cross-compilation ([9c5e3c5](https://github.com/SatoshiAndKin/create2-salt-miner/commit/9c5e3c54c5d957d3344009228c11028ba6c2bf5b))
* add remote mining server ([ed2f216](https://github.com/SatoshiAndKin/create2-salt-miner/commit/ed2f216822bd2bde8d31fef0cf99c6377419ddd4))
* estimate time to next score ([3a3ec4a](https://github.com/SatoshiAndKin/create2-salt-miner/commit/3a3ec4a9488c937ae63e7a306c91594220ff9a78))


### Bug Fixes

* clarify zero-byte mining target ([e48d582](https://github.com/SatoshiAndKin/create2-salt-miner/commit/e48d5821b2457da17e7dda001d1006325571ce6d))
* clean up miner error handling ([8df95ef](https://github.com/SatoshiAndKin/create2-salt-miner/commit/8df95efa1807248cef968263db09084a6b776dcc))
* default optional mining flags ([f7f89da](https://github.com/SatoshiAndKin/create2-salt-miner/commit/f7f89dabd56c6cd16925316e30854515970177d6))
* flush OpenCL queue before throttling ([cd38400](https://github.com/SatoshiAndKin/create2-salt-miner/commit/cd384001a23cad7595e4f1b5b53573e0ddb2d665))
* keep mining jobs after disconnect ([e52f3c6](https://github.com/SatoshiAndKin/create2-salt-miner/commit/e52f3c64b87373e3ed5afa599d889aaf6441d8e4))
* preserve display score rows ([0915dec](https://github.com/SatoshiAndKin/create2-salt-miner/commit/0915dece5792f24ef14950f7181bdc0d9230c594))
* raise default mining target ([db9bf70](https://github.com/SatoshiAndKin/create2-salt-miner/commit/db9bf707e464826031bb57b493c629ec1ae70f29))
* report recent mining throughput ([1c36513](https://github.com/SatoshiAndKin/create2-salt-miner/commit/1c365132366dde6bea3e44b436f72b7b383daf42))
* show runtime with mining result ([53a932e](https://github.com/SatoshiAndKin/create2-salt-miner/commit/53a932e022cc3904f75ac7c7bdcff3422878e807))
* update dependency compatibility ([9437baa](https://github.com/SatoshiAndKin/create2-salt-miner/commit/9437baa04d66c6f27c2cd08cb69362da8bacc971))
* use blocking OpenCL reads for mining loop ([4bdb4a1](https://github.com/SatoshiAndKin/create2-salt-miner/commit/4bdb4a1bce1e305e6359703c53079dc50f26c94d))
* use stable Rust toolchain ([f22b053](https://github.com/SatoshiAndKin/create2-salt-miner/commit/f22b053bf397b1a69c0a0bb9c02521a1535399ac))


### Performance Improvements

* batch mining solution readbacks ([4225d74](https://github.com/SatoshiAndKin/create2-salt-miner/commit/4225d742ce5c72970f07c6e3aed12541161f972b))
* pass nonce as scalar kernel argument ([59935af](https://github.com/SatoshiAndKin/create2-salt-miner/commit/59935af2ba2d8a3592f23a3936ee03e0987fa44d))
* pass salt as scalar kernel argument ([d49e123](https://github.com/SatoshiAndKin/create2-salt-miner/commit/d49e12339e8c67a7c3baae33540f5872db8e86e1))
* reduce mining readback frequency ([1860d98](https://github.com/SatoshiAndKin/create2-salt-miner/commit/1860d98bdd38fcb9b95b4cd3b348e7f1868472f0))
* rely on blocking solution reads ([6e113b7](https://github.com/SatoshiAndKin/create2-salt-miner/commit/6e113b728722683b13d136983e8e49a4febee0cd))
* reuse mining OpenCL objects across salts ([cb6c793](https://github.com/SatoshiAndKin/create2-salt-miner/commit/cb6c7933c165ca90ded4668b682f428c896d5f65))
* reuse OpenCL kernel in mining loop ([b4fde52](https://github.com/SatoshiAndKin/create2-salt-miner/commit/b4fde52c228982e45e2bff54f9b2f688c2d7dc84))
