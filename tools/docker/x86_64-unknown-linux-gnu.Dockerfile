FROM rustembedded/cross:x86_64-unknown-linux-gnu
RUN dpkg --add-architecture x86_64
RUN apt-get update && apt-get install --assume-yes libssl-dev:x86_64
