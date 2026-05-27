# Invalid subcommand error

```console
$ locald invalidcommand
? 2
error: unrecognized subcommand 'invalidcommand'

Usage: locald [OPTIONS] <COMMAND>

For more information, try '--help'.

```

# Missing required argument

```console
$ locald logs --follow --no-follow
? 2
error: unexpected argument '--no-follow' found

  tip: a similar argument exists: '--follow'

Usage: locald logs --follow [SERVICE]

For more information, try '--help'.

```

# Runtime CLI errors render without crash report

```console
$ locald try
? 1
locald::cli::error

  × Empty command


```
