package main

import (
	"os"

	"github.com/nijaru/tk/cmd"
)

// version is set at build time via -ldflags "-X main.version=vX.Y.Z"
var version = "dev"

func main() {
	os.Exit(cmd.Run(os.Args[1:], version))
}
