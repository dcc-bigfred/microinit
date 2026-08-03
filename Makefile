# microinit — build / cross-compile / man pages

TARGET_HOST ?=
TARGET_MUSL ?= aarch64-unknown-linux-musl
CARGO ?= cargo
RUSTUP_TOOLCHAIN ?= stable
export RUSTUP_TOOLCHAIN

PREFIX ?= /usr
MANDIR ?= $(PREFIX)/share/man

.PHONY: all build release release-musl check test test-release-assertions clean man install-man fmt clippy

all: build

build:
	$(CARGO) build

release:
	$(CARGO) build --release

release-musl:
	RUSTFLAGS='-C target-feature=+crt-static' \
		$(CARGO) build --release --target $(TARGET_MUSL)
	@mkdir -p dist
	cp -f target/$(TARGET_MUSL)/release/microinit dist/microinit-linux-arm64
	cp -f scripts/early-boot.sh dist/early-boot.sh
	@echo "wrote dist/microinit-linux-arm64"

check:
	$(CARGO) check

test:
	$(CARGO) test

test-release-assertions:
	$(CARGO) test --profile release-assertions

fmt:
	$(CARGO) fmt

clippy:
	$(CARGO) clippy --all-targets -- -D warnings

clean:
	$(CARGO) clean
	rm -f man/man5/*.gz man/man8/*.gz

# Compress man pages (gzip -n for reproducible builds)
man:
	@for f in man/man5/*.mdoc man/man8/*.mdoc; do \
		[ -f "$$f" ] || continue; \
		out="$${f%.mdoc}.gz"; \
		gzip -n -c "$$f" > "$$out"; \
		echo "wrote $$out"; \
	done

install-man: man
	install -d $(DESTDIR)$(MANDIR)/man5 $(DESTDIR)$(MANDIR)/man8
	install -m 644 man/man5/*.gz $(DESTDIR)$(MANDIR)/man5/
	install -m 644 man/man8/*.gz $(DESTDIR)$(MANDIR)/man8/

# Optional: lint mdoc if mandoc is available
man-lint:
	@command -v mandoc >/dev/null || { echo "mandoc not installed"; exit 1; }
	mandoc -Tlint man/man5/*.mdoc man/man8/*.mdoc
