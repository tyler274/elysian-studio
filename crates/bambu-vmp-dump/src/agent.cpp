// Call C++ std::string NetworkAgent entry points after dlopen so the plugin
// can mint the runtime keypair the way official Studio does.
//
// HTTP completions are posted through bambu_network_set_queue_on_main_fn.
// Sleeping without pumping that queue leaves app_private_key nullptr.

#include <dlfcn.h>

#include <atomic>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <functional>
#include <map>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

namespace {

std::mutex g_mu;
std::vector<std::function<void()>> g_jobs;
std::atomic<int> g_posted{0};

void queue_on_main(std::function<void()> cb) {
    g_posted.fetch_add(1);
    std::lock_guard<std::mutex> lock(g_mu);
    g_jobs.push_back(std::move(cb));
}

int pump_once() {
    std::vector<std::function<void()>> batch;
    {
        std::lock_guard<std::mutex> lock(g_mu);
        batch.swap(g_jobs);
    }
    for (auto &job : batch) {
        try {
            job();
        } catch (...) {
        }
    }
    return static_cast<int>(batch.size());
}

int wait_secs() {
    if (const char *env = std::getenv("BAMBU_VMP_WAIT_SECS")) {
        int n = std::atoi(env);
        if (n > 0 && n < 120) {
            return n;
        }
    }
    return 15;
}

void log_rc(const char *name, int rc) {
    std::fprintf(stderr, "%s rc=%d\n", name, rc);
}

}  // namespace

extern "C" int vmp_init_agent(void *handle, const char *config_dir, const char *cert_folder) {
    if (!handle || !config_dir) {
        return -1;
    }
    using Create = void *(*)(std::string);
    using SetDir = int (*)(void *, std::string);
    using SetCert = int (*)(void *, std::string, std::string);
    using InitLog = int (*)(void *);
    using SetCountry = int (*)(void *, std::string);
    using Start = int (*)(void *);
    using SetQueue = int (*)(void *, std::function<void(std::function<void()>)>);
    using StartDiscovery = bool (*)(void *, bool, bool);
    using Connect = int (*)(void *);
    using UpdateCert = int (*)(void *);
    using SetHeaders = int (*)(void *, std::map<std::string, std::string>);
    using IsLogin = bool (*)(void *);
    using SetHttpErr = int (*)(void *, std::function<void(unsigned, std::string)>);

    auto create = reinterpret_cast<Create>(dlsym(handle, "bambu_network_create_agent"));
    auto set_dir = reinterpret_cast<SetDir>(dlsym(handle, "bambu_network_set_config_dir"));
    auto set_cert = reinterpret_cast<SetCert>(dlsym(handle, "bambu_network_set_cert_file"));
    auto init_log = reinterpret_cast<InitLog>(dlsym(handle, "bambu_network_init_log"));
    auto set_cc = reinterpret_cast<SetCountry>(dlsym(handle, "bambu_network_set_country_code"));
    auto set_queue = reinterpret_cast<SetQueue>(dlsym(handle, "bambu_network_set_queue_on_main_fn"));
    auto start = reinterpret_cast<Start>(dlsym(handle, "bambu_network_start"));
    auto start_discovery =
        reinterpret_cast<StartDiscovery>(dlsym(handle, "bambu_network_start_discovery"));
    auto connect = reinterpret_cast<Connect>(dlsym(handle, "bambu_network_connect_server"));
    auto update_cert = reinterpret_cast<UpdateCert>(dlsym(handle, "bambu_network_update_cert"));
    auto set_headers =
        reinterpret_cast<SetHeaders>(dlsym(handle, "bambu_network_set_extra_http_header"));
    auto is_login = reinterpret_cast<IsLogin>(dlsym(handle, "bambu_network_is_user_login"));
    auto set_http_err =
        reinterpret_cast<SetHttpErr>(dlsym(handle, "bambu_network_set_on_http_error_fn"));
    if (!create) {
        return -2;
    }
    void *agent = nullptr;
    try {
        agent = create(std::string(config_dir));
    } catch (...) {
        return -3;
    }
    if (!agent) {
        return -4;
    }

    std::atomic<bool> run_pump{true};
    std::thread pump_thread([&] {
        while (run_pump.load(std::memory_order_relaxed)) {
            pump_once();
            std::this_thread::sleep_for(std::chrono::milliseconds(10));
        }
    });

    try {
        if (set_dir) {
            log_rc("set_config_dir", set_dir(agent, std::string(config_dir)));
        }
        if (init_log) {
            log_rc("init_log", init_log(agent));
        }
        if (set_cert && cert_folder && cert_folder[0] != '\0') {
            log_rc("set_cert_file",
                   set_cert(agent, std::string(cert_folder), std::string("slicer_base64.cer")));
        }
        if (set_headers) {
            std::map<std::string, std::string> headers;
            headers.emplace("X-BBL-Client-Type", "slicer");
            headers.emplace("X-BBL-Client-Name", "BambuStudio");
            headers.emplace("X-BBL-OS-Type", "linux");
            headers.emplace("X-BBL-Language", "en");
            log_rc("set_extra_http_header", set_headers(agent, headers));
        }
        if (set_cc) {
            log_rc("set_country_code", set_cc(agent, std::string("US")));
        }
        if (set_http_err) {
            set_http_err(agent, [](unsigned code, std::string) {
                std::fprintf(stderr, "http error status=%u\n", code);
            });
        }
        if (set_queue) {
            log_rc("set_queue_on_main_fn", set_queue(agent, queue_on_main));
        }
        if (start) {
            log_rc("start", start(agent));
        }
        if (start_discovery) {
            std::fprintf(stderr, "start_discovery rc=%d\n",
                         start_discovery(agent, true, false) ? 1 : 0);
        }
        if (connect) {
            log_rc("connect_server", connect(agent));
        }
        if (update_cert) {
            log_rc("update_cert", update_cert(agent));
        }
    } catch (...) {
        run_pump.store(false, std::memory_order_relaxed);
        pump_thread.join();
        return -5;
    }

    const int wait = wait_secs();
    std::fprintf(stderr, "pumping main queue for %ds\n", wait);
    std::this_thread::sleep_for(std::chrono::seconds(wait));
    if (update_cert) {
        log_rc("update_cert_again", update_cert(agent));
    }
    run_pump.store(false, std::memory_order_relaxed);
    pump_thread.join();
    pump_once();
    std::fprintf(stderr, "main-queue posts=%d\n", g_posted.load());
    if (is_login) {
        std::fprintf(stderr, "is_user_login=%d\n", is_login(agent) ? 1 : 0);
    }
    return 0;
}
