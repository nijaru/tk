BINARY_NAME=tk
VERSION?=dev

.PHONY: fmt vet build test clean install release-test

fmt:
	@files="$$(git ls-files '*.go')"; \
	if [ -z "$$files" ]; then \
		echo "no tracked Go files to format"; \
	else \
		goimports -w $$files; \
		golines --base-formatter gofumpt -w $$files; \
	fi

vet:
	go vet ./...

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
