SDK ?= $(SUPERPOWERED_SDK_DIR)
CXX ?= clang++
CXXFLAGS = -std=c++17 -O2 -arch arm64 -I$(SDK)/Superpowered
SUPERPOWERED_LIB = $(SDK)/Superpowered/libSuperpoweredAudio.xcframework/macos-arm64_x86_64/libSuperpoweredAudioOSX.a
LIBS = $(SUPERPOWERED_LIB) \
       -framework AudioToolbox \
       -framework CoreMedia \
       -framework CoreAudio \
       -framework AVFoundation \
       -framework CoreFoundation \
       -framework Foundation

sp-bench: main.cpp metrics.h
	$(CXX) $(CXXFLAGS) main.cpp $(LIBS) -o sp-bench
