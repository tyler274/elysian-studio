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
    using GetUid = std::string (*)(void *);
    using GetCam = int (*)(void *, std::string, std::function<void(std::string)>);
    using GetPrint = int (*)(void *, unsigned int *, std::string *);

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
    auto get_uid = reinterpret_cast<GetUid>(dlsym(handle, "bambu_network_get_user_id"));
    auto get_cam = reinterpret_cast<GetCam>(dlsym(handle, "bambu_network_get_camera_url"));
    auto get_print = reinterpret_cast<GetPrint>(dlsym(handle, "bambu_network_get_user_print_info"));
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
            headers.emplace("X-BBL-Client-Version", "02.08.02.61");
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
    std::fprintf(stderr, "pumping main queue for %ds then camera mint\n", wait);
    if (wait > 2) {
        std::this_thread::sleep_for(std::chrono::seconds(2));
    } else {
        std::this_thread::sleep_for(std::chrono::seconds(wait));
    }
    if (update_cert) {
        log_rc("update_cert_again", update_cert(agent));
    }
    if (get_uid) {
        try {
            std::string uid = get_uid(agent);
            int prefix_u = uid.rfind("u_", 0) == 0 ? 1 : 0;
            std::fprintf(stderr, "user_id_len=%zu prefix_u=%d digits=%d\n", uid.size(), prefix_u,
                         uid.find_first_not_of("0123456789") == std::string::npos ? 1 : 0);
        } catch (...) {
            std::fprintf(stderr, "get_user_id threw\n");
        }
    }
    if (is_login) {
        std::fprintf(stderr, "is_user_login=%d\n", is_login(agent) ? 1 : 0);
    }
    if (get_print) {
        unsigned http = 0;
        std::string body;
        int rc = get_print(agent, &http, &body);
        int devices = 0;
        for (size_t i = 0; (i = body.find("\"dev_id\"", i)) != std::string::npos; i++) {
            devices++;
        }
        std::fprintf(stderr, "get_user_print_info rc=%d http=%u body_len=%zu dev_id_fields=%d\n", rc,
                     http, body.size(), devices);
    }
    std::atomic<bool> camera_done{false};
    if (get_cam) {
        const char *key = std::getenv("BAMBU_VMP_CAMERA_KEY");
        if (key && key[0] != '\0') {
            int pipes = 0;
            bool agora = std::string(key).find("agora") != std::string::npos;
            bool tutk = std::string(key).find("tutk") != std::string::npos;
            for (const char *p = key; *p; ++p) {
                if (*p == '|') {
                    pipes++;
                }
            }
            std::fprintf(stderr, "get_camera_url pipes=%d agora=%d tutk=%d\n", pipes, agora ? 1 : 0,
                         tutk ? 1 : 0);
            try {
                int rc = get_cam(agent, std::string(key), [&](std::string url) {
                    const char *scheme = "other";
                    if (url.empty()) {
                        scheme = "empty";
                    } else if (url.rfind("bambu:///agora", 0) == 0) {
                        scheme = "agora";
                    } else if (url.rfind("bambu:///tutk", 0) == 0) {
                        scheme = "tutk";
                    } else if (url.rfind("bambu:///local", 0) == 0) {
                        scheme = "local";
                    } else if (url.find("fail") != std::string::npos || url.find('[') != std::string::npos) {
                        scheme = "error";
                    }
                    int code = 0;
                    auto lb = url.rfind('[');
                    auto rb = url.rfind(']');
                    if (lb != std::string::npos && rb != std::string::npos && rb > lb) {
                        code = std::atoi(url.substr(lb + 1, rb - lb - 1).c_str());
                    }
                    std::fprintf(stderr,
                                 "camera_url scheme=%s len=%zu bracket_code=%d has_channel=%d "
                                 "has_token=%d has_uid=%d\n",
                                 scheme, url.size(), code,
                                 url.find("channel=") != std::string::npos ? 1 : 0,
                                 url.find("token=") != std::string::npos ? 1 : 0,
                                 url.find("uid=") != std::string::npos ? 1 : 0);
                    camera_done.store(true, std::memory_order_relaxed);
                });
                log_rc("get_camera_url", rc);
            } catch (...) {
                std::fprintf(stderr, "get_camera_url threw\n");
            }
            for (int i = 0; i < 250 && !camera_done.load(std::memory_order_relaxed); ++i) {
                std::this_thread::sleep_for(std::chrono::milliseconds(100));
            }
            if (!camera_done.load(std::memory_order_relaxed)) {
                std::fprintf(stderr, "camera_url callback timeout\n");
            }
        }
    }
    run_pump.store(false, std::memory_order_relaxed);
    pump_thread.join();
    pump_once();
    std::fprintf(stderr, "main-queue posts=%d\n", g_posted.load());
    return 0;
}
