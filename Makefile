BINARY_NAME=tk
VERSION?=dev

.PHONY: build test clean install release-test

build:
	go build -ldflags "-X main.version=$(VERSION)" -o $(BINARY_NAME) .

test:
	go test -v ./...

install:
	go install -ldflags "-X main.version=$(VERSION)" .

clean:
	rm -f $(BINARY_NAME)
	rm -rf dist/

# Dry run GoReleaser snapshot (requires: brew install goreleaser)
release-test:
	goreleaser release --snapshot --clean
