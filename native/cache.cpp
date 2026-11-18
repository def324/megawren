#include "megawren/src/bridge.rs.h"
#include <sodium.h>
#include <array>
#include <stdexcept>
#include <algorithm>
namespace megawren {
namespace {
constexpr unsigned char aad[] = "megawren/cache/v1";
constexpr size_t maxChunk = 1024 * 1024;
void initialize() {
    if (sodium_init() < 0) throw std::runtime_error("Crypto initialization failed");
}
}
struct CacheCipher::Impl {
    crypto_secretstream_xchacha20poly1305_state state{};
    std::array<unsigned char, crypto_secretstream_xchacha20poly1305_HEADERBYTES> header{};
    bool writing = false;
    bool finished = false;
    ~Impl() { sodium_memzero(&state, sizeof state); }
};
CacheCipher::CacheCipher(rust::Slice<const uint8_t> key, rust::Slice<const uint8_t> header): impl(std::make_unique<Impl>()) {
    initialize();
    if (key.size() != crypto_secretstream_xchacha20poly1305_KEYBYTES)
        throw std::runtime_error("Invalid cache key");
    impl->writing = header.empty();
    if (impl->writing) {
        crypto_secretstream_xchacha20poly1305_init_push(&impl->state, impl->header.data(), key.data());
    } else {
        if (header.size() != impl->header.size()) throw std::runtime_error("Invalid cache header");
        std::copy(header.begin(), header.end(), impl->header.begin());
        if (crypto_secretstream_xchacha20poly1305_init_pull(&impl->state, impl->header.data(), key.data()))
            throw std::runtime_error("Invalid cache header");
    }
}
CacheCipher::~CacheCipher() = default;
rust::Vec<uint8_t> CacheCipher::header() const {
    rust::Vec<uint8_t> out;
    out.reserve(impl->header.size());
    for (auto b: impl->header) out.push_back(b);
    return out;
}
void CacheCipher::encrypt(rust::Slice<const uint8_t> bytes, rust::Slice<uint8_t> output, bool finalChunk) {
    if (!impl->writing || impl->finished || bytes.size() > maxChunk || output.size() != bytes.size() + crypto_secretstream_xchacha20poly1305_ABYTES)
        throw std::runtime_error("Invalid cache encryption state");
    if (crypto_secretstream_xchacha20poly1305_push(&impl->state, output.data(), nullptr,
            bytes.data(), bytes.size(), aad, sizeof aad - 1,
            finalChunk ? crypto_secretstream_xchacha20poly1305_TAG_FINAL : crypto_secretstream_xchacha20poly1305_TAG_MESSAGE))
        throw std::runtime_error("Cache encryption failed");
    impl->finished = finalChunk;
}
bool CacheCipher::decrypt(rust::Slice<const uint8_t> bytes, rust::Slice<uint8_t> output) {
    constexpr auto overhead = crypto_secretstream_xchacha20poly1305_ABYTES;
    if (impl->writing || impl->finished || bytes.size() < overhead || bytes.size() > maxChunk + overhead || output.size() != bytes.size() - overhead)
        throw std::runtime_error("Invalid cache decryption state");
    unsigned char tag = 0;
    if (crypto_secretstream_xchacha20poly1305_pull(&impl->state, output.data(), nullptr, &tag,
            bytes.data(), bytes.size(), aad, sizeof aad - 1) ||
            (tag != crypto_secretstream_xchacha20poly1305_TAG_MESSAGE && tag != crypto_secretstream_xchacha20poly1305_TAG_FINAL)) {
        sodium_memzero(output.data(), output.size());
        impl->finished = true;
        throw std::runtime_error("Damaged local index");
    }
    impl->finished = tag == crypto_secretstream_xchacha20poly1305_TAG_FINAL;
    return impl->finished;
}
rust::Vec<uint8_t> cache_key(rust::Slice<const uint8_t> session) {
    initialize();
    if (session.empty() || session.size() > 60000) throw std::runtime_error("Invalid cache session");
    // The opaque SDK session is high-entropy secret material, not a password.
    std::array<unsigned char, crypto_kdf_KEYBYTES> master{};
    crypto_generichash(master.data(), master.size(), session.data(), session.size(), nullptr, 0);
    rust::Vec<uint8_t> key;
    key.reserve(crypto_secretstream_xchacha20poly1305_KEYBYTES);
    for (size_t i = 0; i < crypto_secretstream_xchacha20poly1305_KEYBYTES; ++i) key.push_back(0);
    const int result = crypto_kdf_derive_from_key(key.data(), key.size(), 1, "MGWCACHE", master.data());
    sodium_memzero(master.data(), master.size());
    if (result) {
        sodium_memzero(key.data(), key.size());
        throw std::runtime_error("Cache key derivation failed");
    }
    return key;
}
std::unique_ptr<CacheCipher> cache_cipher(rust::Slice<const uint8_t> key, rust::Slice<const uint8_t> header) {
    return std::make_unique<CacheCipher>(key, header);
}
}
