#include "easylogging++.h"

INITIALIZE_EASYLOGGINGPP

int main(int argc, char* argv[]) {
    START_EASYLOGGINGPP(argc, argv);

    el::Configurations conf;
    conf.setToDefault();
    conf.setGlobally(el::ConfigurationType::ToFile, "false");
    conf.setGlobally(el::ConfigurationType::ToStandardOutput, "true");
    el::Loggers::reconfigureAllLoggers(conf);

    LOG(INFO)    << "crane built this against easyloggingpp via a git dep";
    LOG(DEBUG)   << "debug message";
    LOG(WARNING) << "warning message";

    return 0;
}
