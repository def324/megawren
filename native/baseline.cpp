// Offline SDK lifecycle baseline. Never logs in or reads credentials.
#include <megaapi.h>
int main(int argc, char** argv) {
    if (argc != 2) return 2;
    mega::MegaApi::setLogLevel(-1);
    mega::MegaApi::setLogToConsole(false);
    mega::MegaApi api(nullptr, argv[1], "megawren-baseline", 1);
    api.disableGfxFeatures(true);
    return 0;
}
