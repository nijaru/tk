package main

import (
	"os"

	"github.com/nijaru/tk/cmd"
)

func main() {
	os.Exit(cmd.Run(os.Args[1:]))
}
