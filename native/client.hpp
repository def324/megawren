#pragma once
#include "rust/cxx.h"
#include <memory>
namespace megawren {
struct Entry; struct Event; struct Account; struct Metadata; struct FolderStats;
class Client {
    struct Impl;
    std::unique_ptr<Impl> impl;
public:
    explicit Client(rust::Str base);
    ~Client();
    void login(rust::Str email, rust::Str password, rust::Str otp);
    void restore(rust::Str session);
    void fetch();
    void fetch_current();
    bool needs_checkpoint() const;
    Event event();
    void refresh_account();
    Account account();
    int32_t connection();
    rust::String session();
    uint64_t root();
    uint64_t rubbish();
    uint64_t generation() const;
    rust::String upload_location(uint64_t id);
    Metadata metadata(uint64_t id);
    void start_stats(uint64_t id);
    FolderStats folder_stats();
    rust::Vec<Entry> folders(uint64_t id, size_t offset, size_t limit);
    void change(uint8_t kind, uint64_t source, uint64_t parent, rust::Str name, uint64_t generation);
    void change_many(uint8_t kind, rust::Slice<const uint64_t> sources, rust::Slice<const uint64_t> parents, rust::Slice<const rust::String> names, uint64_t generation);
    Entry info(uint64_t id);
    rust::Vec<Entry> children(uint64_t id, size_t offset, size_t limit);
    void mkdir(uint64_t parent, rust::Str name);
    uint64_t result_node();
    void public_file(rust::Str link);
    void public_folder(rust::Str link);
    Entry public_entry();
    void download(uint64_t id, rust::Str path);
    rust::Vec<Entry> named_children(uint64_t parent, rust::Str name, size_t offset);
    bool exists(uint64_t parent,rust::Str name);
    rust::String errors();
    rust::String email();
    void preview(uint64_t id, int32_t fd, int64_t length);
    Event preview_event();
    void cancel_preview();
    void upload(rust::Str path, uint64_t parent, rust::Str name,rust::Str expected);
    void cancel(bool revokeSession);
    void logout();
};
std::unique_ptr<Client> client(rust::Str base);
rust::String utc_date(int64_t timestamp);
}
