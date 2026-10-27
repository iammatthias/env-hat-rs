#!/usr/bin/env bash
# Checks the Markdown docs for the style rules a script can check: words
# the Google developer style guide's word list bans, exclamation marks,
# headings that are not sentence case, and relative links whose file or
# #anchor does not exist. Text in fenced code blocks and inline code is
# exempt. ci/run.sh runs it, and so can you:
#
#   bash ci/docs-lint.sh              # every tracked Markdown file
#   bash ci/docs-lint.sh docs/api.md  # only the files you name
#
# Each problem prints as `file:line: rule: text`. It needs bash and perl.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ $# -eq 0 ]; then
  set -- $(git ls-files '*.md')
fi

exec perl -CSDA -Mutf8 -e '
use strict;
use warnings;
use File::Basename qw(dirname);
use File::Spec;

# Words a heading may capitalize after its first word, and lowercase names
# a heading may start with. All-caps tokens such as BME280 or API always pass.
my %proper = map { $_ => 1 } qw(
  env-hat Hat Adafruit Bosch Debian GitHub Linux Mac Pi Raspberry Rust
  Sensirion Unix Waveshare
);

my @words = (
  [qr/\b(?:[Ww]e|[Uu]s|[Oo]ur)\b/, "first person", "address the reader as \"you\", or name the actor"],
  [qr/\bclick here\b/i, "click here", "make the link text say what the reader gets"],
  [qr/\bplease\b/i, "please", "delete it"],
  [qr/\bsimply\b/i, "simply", "delete it"],
  [qr/\bjust\b/i, "just", "delete it, or write \"only\""],
  [qr/\beas(?:y|ily)\b/i, "easy", "delete it, or say what makes it fast"],
  [qr/\be\.g\./i, "e.g.", "write \"for example\" or \"such as\""],
  [qr/\bi\.e\./i, "i.e.", "write \"that is\""],
  [qr/\betc\./i, "etc.", "say up front that the list is partial"],
  [qr/\bvia\b/i, "via", "write \"through\", \"by\" or \"with\""],
  [qr/!/, "exclamation mark", "end the sentence with a period"],
);

my $failures = 0;
my %anchor_cache;

sub report {
  my ($file, $line, $rule, $text) = @_;
  print "$file:$line: $rule: $text\n";
  $failures++;
}

# Lines outside fenced code blocks, as [line number, text].
sub prose_lines {
  my ($file) = @_;
  open my $fh, "<", $file or die "docs-lint: cannot read $file: $!\n";
  my ($fence, @lines) = (undef);
  while (my $line = <$fh>) {
    chomp $line;
    if ($line =~ /^\s*(```|~~~)/) {
      if (!defined $fence) { $fence = $1 } elsif ($1 eq $fence) { $fence = undef }
      next;
    }
    push @lines, [$., $line] unless defined $fence;
  }
  return @lines;
}

sub strip_code {
  my ($text) = @_;
  $text =~ s/(`+).+?\1/ CODE /g;
  return $text;
}

# GitHub heading anchors: lowercase, keep word characters, hyphens and
# spaces, turn spaces into hyphens, and number repeats.
sub anchors {
  my ($file) = @_;
  return $anchor_cache{$file} if $anchor_cache{$file};
  my (%seen, %anchors);
  for (prose_lines($file)) {
    next unless $_->[1] =~ /^#{1,6}\s+(.*?)\s*#*\s*$/;
    my $slug = lc $1;
    $slug =~ s/[^\w\- ]//g;
    $slug =~ s/ /-/g;
    my $n = $seen{$slug}++;
    $anchors{$n ? "$slug-$n" : $slug} = 1;
  }
  return $anchor_cache{$file} = \%anchors;
}

sub check_link {
  my ($file, $line, $target) = @_;
  return if $target =~ /^[a-z][a-z0-9+.-]*:/i;
  my ($path, $anchor) = $target =~ /^([^#]*)(?:#(.*))?$/;
  my $dest = $path eq "" ? $file : File::Spec->catfile(dirname($file), $path);
  if (!-e $dest) {
    report($file, $line, "broken link", "$target: $dest does not exist");
  } elsif (defined $anchor && $dest =~ /\.md$/ && !anchors($dest)->{$anchor}) {
    report($file, $line, "broken link", "$target: no heading in $dest has the anchor #$anchor");
  }
}

sub check_heading {
  my ($file, $line, $text, $raw) = @_;
  $text =~ s/\[([^\]]*)\]\([^)]*\)/$1/g;
  my @tokens = grep { length } map { s/^[^\w`-]+|[^\w`-]+$//gr } split /\s+/, $text;
  for my $i (0 .. $#tokens) {
    my $t = $tokens[$i];
    next if $t eq "CODE" || $proper{$t};
    if ($i == 0 && $t =~ /^[a-z]/) {
      report($file, $line, "heading case", "start with a capital letter, or add the name to the allowlist in ci/docs-lint.sh: $raw");
    } elsif ($i > 0 && $t =~ /^[A-Z][a-z]/) {
      report($file, $line, "heading case", "use sentence case for \"$t\", or add the proper noun to the allowlist in ci/docs-lint.sh: $raw");
    }
  }
}

for my $file (@ARGV) {
  for (prose_lines($file)) {
    my ($n, $raw) = @$_;
    my $text = strip_code($raw);
    $text =~ s/<!--.*?-->//g;
    while ($text =~ /\[[^\]]*\]\(([^)\s]+)(?:\s+"[^"]*")?\)/g) { check_link($file, $n, $1) }
    check_link($file, $n, $1) if $text =~ /^\s*\[[^\]]+\]:\s*(\S+)/;
    check_heading($file, $n, $1, $raw) if $text =~ /^#{1,6}\s+(.*?)\s*#*\s*$/;
    my $prose = $text =~ s/!?\[([^\]]*)\]\([^)]*\)/$1/gr =~ s/^\s*\[[^\]]+\]:\s*\S+.*//r;
    for (@words) {
      my ($re, $rule, $fix) = @$_;
      report($file, $n, $rule, "$fix: $raw") if $prose =~ $re;
    }
  }
}

if ($failures) {
  print "docs-lint: $failures problem(s)\n";
  exit 1;
}
print "docs-lint: ", scalar(@ARGV), " file(s) clean\n";
' "$@"
