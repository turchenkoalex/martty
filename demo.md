# martty

A tiny **Markdown** renderer for *Ghostty*, with `inline code`,
~~strikethrough~~ and [links](https://ghostty.org) that are clickable (OSC 8).

## Images

![demo gradient](assets/demo.png)

> Images in quotes keep the bar:
>
> ![small](assets/demo.png)

![missing image falls back to alt text](assets/nope.png)

## Diagrams

```mermaid
flowchart LR
    A[Markdown] --> B{Mermaid?}
    B -->|yes| C[mmdr]
    B -->|no| D[Code block]
    C --> E[resvg] --> F((Ghostty))
    D --> F
```

```mermaid
sequenceDiagram
    participant M as martty
    participant G as Ghostty
    M->>G: query caps
    G-->>M: OSC 11, kitty OK
    M->>G: image (kitty graphics)
    Note over M,G: no browser needed
```

```mermaid
pie title Where time goes
    "Layout" : 45
    "Rasterize" : 35
    "Transmit" : 20
```

## Lists

- first item with a fairly long line of text that should wrap nicely under the bullet instead of under the margin
- second item
  - nested **bold** item
  - another one
- [ ] todo
- [x] done

1. one
2. two
3. three

### Code

```rust
fn main() {
    println!("hello, ghostty");
}
```

> Block quotes get a bar on the left.
>
> Even across *paragraphs*.

| Name   | Kind     | Size |
|--------|:--------:|-----:|
| martty  | binary   |  1 MB |
| ghostty | terminal | 30 MB |

---

#### Footnotes

Some claim[^1].

[^1]: The footnote.
