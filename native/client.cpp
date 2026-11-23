#include <optional>
#include "megawren/src/bridge.rs.h"
#include <megaapi.h>
#include <sodium.h>
#include <mutex>
#include <stdexcept>
#include <cstring>
#include <atomic>
#include <chrono>
#include <cstdio>
#include <vector>
#include <set>
#include <map>
#ifndef _WIN32
#include <unistd.h>
#include <fcntl.h>
#include <cerrno>
#endif
using namespace mega;
namespace megawren {
namespace {
struct Secret {
    std::string value;
    explicit Secret(rust::Str s): value(s) {}
    ~Secret() { if (!value.empty()) sodium_memzero(value.data(), value.size()); }
};
LinkState directLink(MegaNode& n) {
    if (!n.isExported()) return LinkState::None;
    if (n.isTakenDown()) return LinkState::TakenDown;
    if (n.isExpired()) return LinkState::Expired;
    return n.getPublicHandle() == INVALID_HANDLE ? LinkState::Unknown : LinkState::Active;
}
Entry entry(MegaApi& api, MegaNode& n) {
    const char* name = n.getName();
    if (!name || std::strlen(name) > 32768) throw std::runtime_error("Invalid remote name");
    const char* fingerprint = n.getFingerprint();
    return {n.getHandle(), rust::String(name), n.getSize(), n.isFolder(), n.getModificationTime(), n.isShared() || (n.isFolder() && api.isPendingShare(&n)), n.isExported(), n.getParentHandle(), rust::String(fingerprint ? fingerprint : "")};
}
}
struct Client::Impl : MegaRequestListener, MegaTransferListener, MegaGlobalListener {
    struct PreviewListener : MegaTransferListener {
        std::mutex mutex;
        Event state{true,0,0,0,0};
        int fd=-1,tag=-1;
        bool cancelled=false;
        ~PreviewListener() {
#ifndef _WIN32
            if(fd>=0) ::close(fd);
#endif
        }
        void onTransferStart(MegaApi* api,MegaTransfer* t) override {
            bool stop;
            {std::lock_guard lock(mutex);tag=t->getTag();stop=cancelled;}
            if(stop) api->cancelTransferByTag(t->getTag());
        }
        bool onTransferData(MegaApi*,MegaTransfer*,char* buffer,size_t count) override {
            std::lock_guard lock(mutex);
            if(cancelled) return false;
            if(count>static_cast<uint64_t>(state.total-state.bytes)) {state.code=MegaError::API_EINTERNAL;return false;}
#ifndef _WIN32
            size_t offset=0;
            while(offset<count) {
                auto n=::write(fd,buffer+offset,count-offset);
                if(n<0 && errno==EINTR) continue;
                if(n<=0) {state.code=MegaError::API_EWRITE;return false;}
                offset+=static_cast<size_t>(n);
            }
            state.bytes+=count;
            return true;
#else
            return false;
#endif
        }
        void onTransferTemporaryError(MegaApi*,MegaTransfer*,MegaError* e) override {std::lock_guard lock(mutex);state.retry=e->getErrorCode();}
        void onTransferFinish(MegaApi*,MegaTransfer*,MegaError* e) override {
            std::lock_guard lock(mutex);
            if(!state.code) state.code=cancelled?MegaError::API_EINCOMPLETE:e->getErrorCode();
            if(!state.code && state.bytes!=state.total) state.code=MegaError::API_EINTERNAL;
            state.done=true;
#ifndef _WIN32
            if(fd>=0) ::close(fd);
#endif
            fd=-1;tag=-1;
        }
    } previewListener;
    std::atomic<uint64_t> generation{0};
    std::atomic<bool> nodesCurrent{false};
    // Sticky across failed and cancelled writes: the server may already have applied them.
    std::atomic<bool> needsCheckpoint{false};
    bool waitForCurrent = false; // protected by mutex
    void onNodesUpdate(MegaApi*, MegaNodeList*) override { generation.fetch_add(1); }
    void onEvent(MegaApi*, MegaEvent* event) override {
        if (event->getType() == MegaEvent::EVENT_NODES_CURRENT) nodesCurrent.store(true);
        if (event->getType() == MegaEvent::EVENT_RELOADING) {
            nodesCurrent.store(false);
            needsCheckpoint.store(true);
        }
    }
    struct StatsListener : MegaRequestListener {
        std::mutex mutex;
        FolderStats state{false,false,0,0,0,0,0,0};
        void onRequestFinish(MegaApi*, MegaRequest* r, MegaError* e) override {
            std::lock_guard lock(mutex);
            state.pending=false; state.code=e->getErrorCode();
            if (state.code) return;
            auto* info=r->getMegaFolderInfo(); // borrowed until this callback returns
            if (!info) {state.code=MegaError::API_EINTERNAL;return;}
            state.files=info->getNumFiles(); state.folders=info->getNumFolders();
            state.bytes=info->getCurrentSize(); state.versions=info->getNumVersions(); state.version_bytes=info->getVersionsSize();
            state.ready=state.files>=0 && state.folders>=0 && state.bytes>=0 && state.versions>=0 && state.version_bytes>=0;
            if (!state.ready) state.code=MegaError::API_EINTERNAL;
        }
    } statsListener;
    struct AccountListener : MegaRequestListener {
        std::mutex mutex;
        Account state{false,false,0,0,0};
        void onRequestFinish(MegaApi*, MegaRequest* r, MegaError* e) override {
            std::lock_guard lock(mutex);
            state.pending=false; state.code=e->getErrorCode();
            if (state.code) return;
            try {
                std::unique_ptr<MegaAccountDetails> details(r->getMegaAccountDetails());
                if (!details) {state.code=MegaError::API_EINTERNAL;return;}
                state.used=details->getStorageUsed(); state.total=details->getStorageMax();
                if (state.used<0 || state.total<=0) {state.ready=false;state.code=MegaError::API_EINTERNAL;return;}
                state.ready=true;
            } catch (...) {state.code=MegaError::API_EINTERNAL;}
        }
    } accountListener;
    std::mutex mutex;
    Event state{true,0,0,0,0};
    size_t requests = 1;
    bool batch = false;
    bool transfer = false;
    bool cancelling = false;
    uint64_t node = INVALID_HANDLE;

    struct Publication {
        uint64_t parent;
        std::string destination, finalName;
    };
    std::map<std::string, Publication> publications;
    std::map<uint64_t, std::string> renaming;
    std::map<int, std::string> requestNames;
    std::string uploadStage, errors;
    std::string destinationPath(MegaNode& parent) {
        if (!parent.isFolder() || (parent.getType()!=MegaNode::TYPE_ROOT && protectedNode(parent)))
            throw std::runtime_error("Choose a private folder in the cloud drive");
        std::unique_ptr<char[]> path(api->getNodePath(&parent));
        if (!path) throw std::runtime_error("Destination unavailable");
        return path.get();
    }
    using Staging = std::pair<std::string, Publication>;
    Staging stage(MegaNode& parent, const std::string& name) {
        unsigned char random[16];
        randombytes_buf(random, sizeof random);
        char hex[33];
        sodium_bin2hex(hex, sizeof hex, random, sizeof random);
        std::string temporary = std::string("megawren-part-") + hex;
        auto occupied = std::unique_ptr<MegaNode>(api->getChildNode(&parent, temporary.c_str()));
        if (occupied) throw std::runtime_error("Temporary name collision");
        Publication publication{parent.getHandle(), destinationPath(parent), name};
        return {std::move(temporary), std::move(publication)};
    }
    void recordError(const std::string& text) {
        if (!errors.empty()) errors += "\n";
        errors += text;
    }
    // Runs on the SDK callback thread. renameNode changes only the name
    // attribute of this handle. Concurrent same-name siblings retain their data.
    bool publish(uint64_t handle, const std::string& temporary) {
        auto publication = publications.at(temporary);
        auto created = get(handle);
        auto parent = get(publication.parent);
        if (created->getParentHandle()!=publication.parent || !created->getName() ||
            temporary!=created->getName() || destinationPath(*parent)!=publication.destination)
            throw std::runtime_error("Destination changed");
        auto occupied = std::unique_ptr<MegaNode>(api->getChildNode(parent.get(), publication.finalName.c_str()));
        if (occupied) throw std::runtime_error("Destination name is in use");
        renaming.emplace(handle, temporary);
        api->renameNode(created.get(), publication.finalName.c_str(), this);
        return true;
    }
    void onRequestStart(MegaApi*, MegaRequest* request) override {
        std::lock_guard lock(mutex);
        if (!batch) return;
        auto source = std::unique_ptr<MegaNode>(api->getNodeByHandle(request->getNodeHandle()));
        if (source && source->getName()) requestNames[request->getTag()] = source->getName();
    }
    std::unique_ptr<MegaNode> publicNode;
    std::unique_ptr<MegaCancelToken> token;
    // Last field: destroyed first, while listeners and mutex are still alive.
    std::unique_ptr<MegaApi> api;
    explicit Impl(rust::Str base) {
        MegaApi::setLogLevel(-1);
        MegaApi::setLogToConsole(false);
        std::string b(base);
        // The SDK keeps the historical constructor parameter for ABI compatibility.
        // No application key is required; pass nullptr rather than an empty key.
        api = std::make_unique<MegaApi>(nullptr, b.c_str(), "megawren/0.1", 1);
        api->disableGfxFeatures(true);
        api->setAutoResumeTransfers(false);
        api->addGlobalListener(this);
        // This client browses direct children; it never uses global search or
        // listAllNodes. Avoid building their numerous indexes for large trees.
        if (api->enableSearchDBIndexes(false) != MegaError::API_OK)
            throw std::runtime_error("Could not configure SDK indexes");
    }
    void begin(bool isTransfer = false) {
        std::lock_guard lock(mutex);
        if (!state.done) throw std::runtime_error("An operation is already running");
        waitForCurrent = false;
        state = {false,0,0,0,0}; node = INVALID_HANDLE;
        publications.clear(); renaming.clear(); requestNames.clear(); uploadStage.clear(); errors.clear();
        transfer = isTransfer; cancelling = false; token.reset(); batch=false; requests=1;
    }
    void onRequestFinish(MegaApi*, MegaRequest* r, MegaError* e) override {
        std::lock_guard lock(mutex);
        // A local logout cancels the original request. Wait for that logout's
        // completion instead of mistaking an earlier callback for quiescence.
        if (cancelling && !transfer && r->getType() != MegaRequest::TYPE_LOGOUT) return;
        try {
            node = r->getNodeHandle();
            if (r->getType() == MegaRequest::TYPE_GET_PUBLIC_NODE && e->getErrorCode() == 0)
                publicNode.reset(r->getPublicMegaNode());
            int code = e->getErrorCode();
            std::string temporary;
            if (r->getType()==MegaRequest::TYPE_COPY && r->getName() && publications.count(r->getName())) {
                temporary = r->getName();
                if (!code) {
                    try { if (publish(node, temporary)) return; }
                    catch (const std::exception& error) {
                        recordError(publications.at(temporary).finalName + ": " + error.what() + ". Copy retained as " + temporary);
                        code = MegaError::API_EEXIST;
                    }
                }
            } else if (auto it=renaming.find(node); it!=renaming.end() && r->getType()==MegaRequest::TYPE_RENAME) {
                temporary=it->second;
                if (code) recordError(publications.at(temporary).finalName + ": retained as " + temporary);
            }
            if (code && batch && temporary.empty() && requestNames.count(r->getTag()))
                recordError(requestNames[r->getTag()] + " (" + std::to_string(code) + ")");
            if (code && batch && r->getType()==MegaRequest::TYPE_COPY && !temporary.empty() && e->getErrorCode())
                recordError(publications.at(temporary).finalName + " (" + std::to_string(code) + ")");
            if(!batch || !state.code) state.code = code;
            if(batch && !code) ++state.bytes;
        } catch (...) { state.code = MegaError::API_EINTERNAL; }
        if(batch && !cancelling) {
            if(--requests) return;
        }
        state.done = true;
    }
    void onRequestUpdate(MegaApi*, MegaRequest* r) override {
        std::lock_guard lock(mutex);
        if(batch || !uploadStage.empty()) return;
        if (r->getType() == MegaRequest::TYPE_FETCH_NODES && r->getTransferredBytes() > 0)
            needsCheckpoint.store(true);
        state.bytes=r->getTransferredBytes(); state.total=r->getTotalBytes(); state.retry=0;
    }
    void onRequestTemporaryError(MegaApi*, MegaRequest*, MegaError* e) override {
        std::lock_guard lock(mutex); state.retry=e->getErrorCode();
    }
    void onTransferTemporaryError(MegaApi*, MegaTransfer*, MegaError* e) override {
        std::lock_guard lock(mutex); state.retry=e->getErrorCode();
    }
    void onTransferUpdate(MegaApi*, MegaTransfer* t) override {
        std::lock_guard lock(mutex);
        state.bytes=t->getTransferredBytes(); state.total=t->getTotalBytes(); state.retry=0;
    }
    void onTransferFinish(MegaApi*, MegaTransfer* t, MegaError* e) override {
        std::lock_guard lock(mutex);
        node=t->getNodeHandle();
        state.bytes=t->getTransferredBytes(); state.total=t->getTotalBytes();
        state.code=e->getErrorCode();
        if (!state.code && !uploadStage.empty()) {
            try { if (publish(node, uploadStage)) return; }
            catch (const std::exception& error) {
                recordError(publications.at(uploadStage).finalName + ": " + error.what() + ". Upload retained as " + uploadStage);
                state.code=MegaError::API_EEXIST;
            }
        }
        state.done=true;
    }
    std::unique_ptr<MegaNode> get(uint64_t id) {
        if (publicNode && publicNode->getHandle() == id) return std::unique_ptr<MegaNode>(publicNode->copy());
        auto n=std::unique_ptr<MegaNode>(api->getNodeByHandle(id));
        if (!n) throw std::runtime_error("Remote item no longer exists");
        return n;
    }
    bool protectedNode(MegaNode& node, bool allowRubbish=false) {
        if (node.getType()!=MegaNode::TYPE_FILE && node.getType()!=MegaNode::TYPE_FOLDER) return true;
        if (api->getAccess(&node)!=MegaShare::ACCESS_OWNER) return true;
        auto current=std::unique_ptr<MegaNode>(node.copy());
        for (int depth=0; current && depth<256; ++depth) {
            if (current->isShared() || current->isExported() || (current->isFolder() && api->isPendingShare(current.get()))) return true;
            if (current->getType()==MegaNode::TYPE_ROOT) return false;
            if (current->getType()==MegaNode::TYPE_RUBBISH) return !allowRubbish;
            current.reset(api->getParentNode(current.get()));
        }
        return true;
    }
    rust::Vec<Entry> list(uint64_t id, size_t offset, size_t limit, bool onlyFolders, const char* name = nullptr) {
        if(limit==0 || limit>256) throw std::runtime_error("Invalid directory page size");
        auto n=get(id);
        if(!n->isFolder()) throw std::runtime_error("Expected a remote folder");
        std::unique_ptr<MegaSearchFilter> filter(MegaSearchFilter::createInstance());
        filter->byLocationHandle(id);
        if(name) filter->byName(name);
        if(onlyFolders) filter->byNodeType(MegaNode::TYPE_FOLDER);
        std::unique_ptr<MegaSearchPage> page(MegaSearchPage::createInstance(offset,limit));
        std::unique_ptr<MegaNodeList> list(api->getChildren(filter.get(),MegaApi::ORDER_DEFAULT_ASC,nullptr,page.get()));
        rust::Vec<Entry> out;
        size_t bytes=0;
        if(list) for(int i=0;i<list->size();++i) {
            auto row=entry(*api, *list->get(i));
            bytes += row.name.size() + sizeof(Entry);
            if(bytes > 1024*1024) throw std::runtime_error("Directory page metadata exceeds safety limit");
            out.push_back(std::move(row));
        }
        return out;
    }
    struct Planned {std::unique_ptr<MegaNode> node,target;std::string name;std::optional<Staging> staged;};
    Planned plan(uint8_t kind,uint64_t source,uint64_t parent,rust::Str name) {
    auto n=get(source);
    if(kind==6 && !api->isInRubbish(n.get())) throw std::runtime_error("Item is no longer in the Rubbish Bin");
    if(kind!=5 && protectedNode(*n,kind==6)) throw std::runtime_error("This action is limited to private items owned by this account");
    std::unique_ptr<MegaNode> target;
    if(kind==4) target.reset(api->getRubbishNode());
    else if(kind==2 || kind==3 || kind==5 || kind==6 || kind==7) target=get(parent);
    if(kind!=1 && !target) throw std::runtime_error("Destination unavailable");
    if(target) {
        if(!target->isFolder()) throw std::runtime_error("Destination must be a folder");
        if(kind!=4) {
            if(target->getType()!=MegaNode::TYPE_ROOT && protectedNode(*target)) throw std::runtime_error("Choose a private folder in the cloud drive");
            auto current=std::unique_ptr<MegaNode>(target->copy());
            for(int depth=0;current && depth<256;++depth) {
                if(kind!=5 && current->getHandle()==source) throw std::runtime_error("A folder cannot be placed inside itself");
                current.reset(api->getParentNode(current.get()));
            }
        }
    }
    std::string value(name);
    if(kind==1 || kind==3 || kind==5 || kind==7) {
        if(value.empty() || value.size()>255 || value=="." || value=="..") throw std::runtime_error("Invalid name");
        for(unsigned char c:value) if(c<32 || c==127 || std::strchr("/\\:<>\"|?*",c)) throw std::runtime_error("Unsafe name");
    }
        if(kind!=4) {
            const char* finalName=(kind==1 || kind==3 || kind==5 || kind==7)?value.c_str():n->getName();
            auto destination=target?std::unique_ptr<MegaNode>(target->copy()):std::unique_ptr<MegaNode>(api->getParentNode(n.get()));
            if(!destination) throw std::runtime_error("Destination unavailable");
            std::unique_ptr<MegaNode> existing(api->getChildNode(destination.get(),finalName));
            if(existing) throw std::runtime_error("An item with that name already exists");
        }
        auto staged = kind == 3 ? std::optional(stage(*target,value)) : std::nullopt;
        return {std::move(n),std::move(target),std::move(value),std::move(staged)};
    }
    void submit(uint8_t kind,Planned& plan) {
        needsCheckpoint.store(true);
        switch(kind) {
            case 1: api->renameNode(plan.node.get(),plan.name.c_str(),this);break;
            case 2: case 4: case 6: api->moveNode(plan.node.get(),plan.target.get(),this);break;
            case 3: {
                const auto& name = plan.staged->first;
                {std::lock_guard lock(mutex); publications.insert(*plan.staged);}
                api->copyNode(plan.node.get(),plan.target.get(),name.c_str(),this);
                break;
            }
            case 5: api->createFolder(plan.name.c_str(),plan.target.get(),this);break;
            case 7: api->moveNode(plan.node.get(),plan.target.get(),plan.name.c_str(),this);break;
            default: throw std::runtime_error("Unknown file action");
        }
    }

};
Client::Client(rust::Str base): impl(std::make_unique<Impl>(base)) {}
Client::~Client() = default;
std::unique_ptr<Client> client(rust::Str b) { return std::make_unique<Client>(b); }
void Client::login(rust::Str e, rust::Str p, rust::Str o) {
    impl->begin(); Secret email(e), pass(p), otp(o);
    if(o.empty()) impl->api->login(email.value.c_str(),pass.value.c_str(),impl.get());
    else impl->api->multiFactorAuthLogin(email.value.c_str(),pass.value.c_str(),otp.value.c_str(),impl.get());
}
void Client::restore(rust::Str s) { impl->begin(); Secret v(s); impl->api->fastLogin(v.value.c_str(),impl.get()); }
void Client::fetch() { impl->begin(); impl->api->fetchNodes(impl.get()); }
void Client::fetch_current() {
    impl->begin();
    { std::lock_guard lock(impl->mutex); impl->waitForCurrent = true; }
    impl->api->fetchNodes(impl.get());
}
bool Client::needs_checkpoint() const { return impl->needsCheckpoint.load(); }
Event Client::event() {
    std::lock_guard lock(impl->mutex);
    auto event = impl->state;
    if (impl->waitForCurrent && event.done && !event.code && !impl->nodesCurrent.load())
        event.done = false;
    return event;
}
void Client::refresh_account() {
    {
        std::lock_guard lock(impl->accountListener.mutex);
        if (impl->accountListener.state.pending) return;
        impl->accountListener.state.pending=true;
    }
    try {impl->api->getSpecificAccountDetails(true,false,false,-1,&impl->accountListener);}
    catch (...) {std::lock_guard lock(impl->accountListener.mutex);impl->accountListener.state.pending=false;throw;}
}
Account Client::account() {std::lock_guard lock(impl->accountListener.mutex);return impl->accountListener.state;}
int32_t Client::connection() {
    if (!impl->api->isLoggedIn()) return 2;
    return impl->api->isOnline() && !impl->api->isWaiting() ? 0 : 1;
}
rust::String Client::session() {
    std::unique_ptr<char[]> s(impl->api->dumpSession());
    if (!s) throw std::runtime_error("No active session");
    rust::String out(s.get()); sodium_memzero(s.get(),std::strlen(s.get())); return out;
}
uint64_t Client::root() { std::unique_ptr<MegaNode> n(impl->api->getRootNode()); if(!n) throw std::runtime_error("No remote root"); return n->getHandle(); }
uint64_t Client::rubbish() {std::unique_ptr<MegaNode> n(impl->api->getRubbishNode());if(!n) throw std::runtime_error("Rubbish Bin unavailable");return n->getHandle();}
uint64_t Client::generation() const {return impl->generation.load();}
rust::String Client::upload_location(uint64_t id) {return rust::String(impl->destinationPath(*impl->get(id)));}
Metadata Client::metadata(uint64_t id) {
    auto epoch=generation();
    auto n=impl->get(id);
    std::unique_ptr<char[]> path(impl->api->getNodePath(n.get()));
    if(!path || std::strlen(path.get())>32768) throw std::runtime_error("Remote path is too long to review safely");
    Metadata out{id,n->getParentHandle(),rust::String(path.get()),n->getCreationTime(),impl->api->getAccess(n.get()),impl->api->isInRubbish(n.get()),n->getRestoreHandle(),epoch,impl->protectedNode(*n,true),n->isFavourite(),directLink(*n)};
    if(epoch!=generation()) throw std::runtime_error("Cloud changed while reading details; try again");
    return out;
}
void Client::start_stats(uint64_t id) {
    auto n=impl->get(id);
    {std::lock_guard lock(impl->statsListener.mutex);if(impl->statsListener.state.pending) throw std::runtime_error("Folder information is still loading");impl->statsListener.state={true,false,0,0,0,0,0,0};}
    impl->api->getFolderInfo(n.get(),&impl->statsListener);
}
FolderStats Client::folder_stats() {std::lock_guard lock(impl->statsListener.mutex);return impl->statsListener.state;}
rust::Vec<Entry> Client::folders(uint64_t id,size_t offset,size_t limit) {return impl->list(id,offset,limit,true);}
void Client::change(uint8_t kind,uint64_t source,uint64_t parent,rust::Str name,uint64_t epoch) {
    if(kind<1 || kind>7) throw std::runtime_error("Unknown file action");
    if(epoch!=generation()) throw std::runtime_error("Cloud changed; review the action again");
    auto plan=impl->plan(kind,source,parent,name);
    if(epoch!=generation()) throw std::runtime_error("Cloud changed; review the action again");
    impl->begin();impl->submit(kind,plan);
}
void Client::change_many(uint8_t kind,rust::Slice<const uint64_t> sources,rust::Slice<const uint64_t> parents,rust::Slice<const rust::String> names,uint64_t epoch) {
    if(kind<2 || kind>6 || kind==5 || sources.empty() || sources.size()>128 || sources.size()!=parents.size() || sources.size()!=names.size()) throw std::runtime_error("Invalid selection");
    if(epoch!=generation()) throw std::runtime_error("Cloud changed; review the action again");
    std::vector<Impl::Planned> plans;
    std::set<uint64_t> ids;
    std::set<std::pair<uint64_t,std::string>> destinations;
    for(size_t i=0;i<sources.size();++i) {
        if(!ids.insert(sources[i]).second) throw std::runtime_error("Duplicate selection");
        plans.push_back(impl->plan(kind,sources[i],parents[i],rust::Str(names[i])));
        auto& plan=plans.back();
        if(kind!=4 && !destinations.insert({parents[i],kind==3?plan.name:std::string(plan.node->getName())}).second) throw std::runtime_error("Selected items have conflicting names");
    }
    if(epoch!=generation()) throw std::runtime_error("Cloud changed; review the action again");
    impl->begin();
    {std::lock_guard lock(impl->mutex);impl->batch=true;impl->requests=plans.size();impl->state.total=plans.size();}
    for(auto& plan:plans) impl->submit(kind,plan);
}
rust::String utc_date(int64_t timestamp) {
    if(timestamp<=0 || timestamp>253402300799LL) return rust::String("Unknown");
    using namespace std::chrono;
    sys_seconds time{seconds(timestamp)};
    auto day=floor<days>(time);
    year_month_day date{day};
    hh_mm_ss clock{time-day};
    char out[32];
    std::snprintf(out,sizeof out,"%04d-%02u-%02u %02lld:%02lld:%02lld UTC",int(date.year()),unsigned(date.month()),unsigned(date.day()),(long long)clock.hours().count(),(long long)clock.minutes().count(),(long long)clock.seconds().count());
    return rust::String(out);
}
Entry Client::info(uint64_t id) { auto n=impl->get(id); return entry(*impl->api, *n); }
rust::Vec<Entry> Client::children(uint64_t id,size_t offset,size_t limit) {
    return impl->list(id,offset,limit,false);
}
rust::Vec<Entry> Client::named_children(uint64_t parent, rust::Str name, size_t offset) {
    const std::string query(name);
    return impl->list(parent, offset, 256, false, query.c_str());
}
void Client::mkdir(uint64_t id, rust::Str name) {
    auto parent = impl->get(id);
    std::string folderName(name);
    impl->destinationPath(*parent);
    impl->begin();
    impl->needsCheckpoint.store(true);
    impl->api->createFolder(folderName.c_str(), parent.get(), impl.get());
}
uint64_t Client::result_node() { std::lock_guard lock(impl->mutex); return impl->node; }
void Client::public_file(rust::Str link) { Secret s(link); impl->begin(); impl->api->getPublicNode(s.value.c_str(),impl.get()); }
void Client::public_folder(rust::Str link) { Secret s(link); impl->begin(); impl->api->loginToFolder(s.value.c_str(),impl.get()); }
Entry Client::public_entry() { std::lock_guard lock(impl->mutex); if(!impl->publicNode) throw std::runtime_error("Invalid public file"); return entry(*impl->api, *impl->publicNode); }
void Client::download(uint64_t id,rust::Str path) {
    auto n=impl->get(id); std::string p(path);
    impl->begin(true); impl->token.reset(MegaCancelToken::createInstance());
    impl->api->startDownload(n.get(),p.c_str(),nullptr,nullptr,false,impl->token.get(),
        MegaTransfer::COLLISION_CHECK_ASSUMEDIFFERENT,MegaTransfer::COLLISION_RESOLUTION_NEW_WITH_N,false,impl.get());
}
bool Client::exists(uint64_t parent,rust::Str name) {
    auto n=impl->get(parent);
    return bool(std::unique_ptr<MegaNode>(impl->api->getChildNode(n.get(),std::string(name).c_str())));
}
rust::String Client::errors() {
    std::lock_guard lock(impl->mutex); return rust::String(impl->errors);
}
rust::String Client::email() {
    std::unique_ptr<char[]> value(impl->api->getMyEmail());
    if(!value) return rust::String();
    size_t length=std::strlen(value.get());
    rust::String out=length<=512?rust::String(value.get()):rust::String();
    sodium_memzero(value.get(),length);return out;
}
void Client::preview(uint64_t id,int32_t fd,int64_t length) {
#ifdef _WIN32
    throw std::runtime_error("Secure preview storage is unavailable on this platform");
#else
    auto n=impl->get(id);
    if(!n->isFile() || n->getSize()<0 || n->getSize()>100LL*1024*1024 || length<=0 || length>n->getSize()) throw std::runtime_error("Preview exceeds its size limit");
    auto& listener=impl->previewListener;
    {std::lock_guard lock(listener.mutex);
        if(!listener.state.done) throw std::runtime_error("Previous preview is stopping");
        int copy=fcntl(fd,F_DUPFD_CLOEXEC,0);
        if(copy<0) throw std::runtime_error("Cannot open private preview storage");
        listener.fd=copy;listener.tag=-1;listener.cancelled=false;listener.state={false,0,0,length,0};
    }
    impl->api->startStreaming(n.get(),0,length,&listener);
#endif
}
Event Client::preview_event() {auto& listener=impl->previewListener;std::lock_guard lock(listener.mutex);return listener.state;}
void Client::cancel_preview() {
    int tag;
    {auto& listener=impl->previewListener;std::lock_guard lock(listener.mutex);if(listener.state.done)return;listener.cancelled=true;tag=listener.tag;}
    if(tag>=0) impl->api->cancelTransferByTag(tag);
}
void Client::upload(rust::Str path,uint64_t parent,rust::Str name,rust::Str expected) {
    auto n=impl->get(parent); std::string p(path),s(name);
    if (std::unique_ptr<MegaNode>(impl->api->getChildNode(n.get(), s.c_str())))
        throw std::runtime_error("Remote file already exists");
    auto staged = impl->stage(*n,s);
    if (staged.second.destination != std::string(expected)) throw std::runtime_error("Upload destination changed");
    MegaUploadOptions options;
    options.fileName = staged.first;
    impl->begin(true);
    { std::lock_guard lock(impl->mutex);
      impl->uploadStage=staged.first;
      impl->publications.insert(std::move(staged)); }
    impl->token.reset(MegaCancelToken::createInstance());
    impl->needsCheckpoint.store(true);
    impl->api->startUpload(p,n.get(),impl->token.get(),&options,impl.get());
}
void Client::cancel(bool revokeSession) {
    bool transfer;
    {
        std::lock_guard lock(impl->mutex);
        impl->waitForCurrent = false;
        if (impl->state.done || impl->cancelling) return;
        impl->cancelling = true;
        transfer = impl->transfer;
    }
    if (transfer && impl->token) impl->token->cancel();
    else if (revokeSession) impl->api->logout(impl.get());
    else impl->api->localLogout(impl.get());
}
void Client::logout() { impl->begin(); impl->api->logout(impl.get()); }
}
