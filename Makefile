BINARY_NAME=tk

.PHONY: build test clean release-test

build:
	go build -o $(BINARY_NAME) .

test:
	go test -v ./...

clean:
	rm -f $(BINARY_NAME)
	rm -rf dist/

# Dry run of GoReleaser
release-test:
	goreleaser release --snapshot --clean
