# Source this before building: toolchain paths for herdc.
export ANDROID_HOME="$HOME/Library/Android/sdk"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/30.0.16248370"
export JAVA_HOME="/Library/Java/JavaVirtualMachines/jdk-21.jdk/Contents/Home"
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$HOME/.local/bin:$ANDROID_HOME/platform-tools:$PATH"
