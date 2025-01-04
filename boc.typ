#import "@preview/polylux:0.3.1": *
#import "@preview/codelst:2.0.2": sourcecode
#import "@preview/diagraph:0.3.0": *
#import "@preview/xarrow:0.3.1": xarrow, xarrowSquiggly, xarrowTwoHead

#set page(paper: "presentation-16-9")
#set text(size: 22pt, font: "Avenir Next LT Pro")

// Page 1
#polylux-slide[
  #set align(horizon + center)
  = Behaviour-Oriented Concurrency

  #link("https://github.com/Foreverhighness")[Hange Shen]

  Jan 5, 2025

  #link("https://github.com/Foreverhighness/boc-talk")[https://github.com/Foreverhighness/boc-talk]
]

// Page 2
#polylux-slide[
  #show link: underline
  #set underline(offset: 8pt)
  == Links

  #link("https://dl.acm.org/doi/10.1145/3622852")[Behaviour-Oriented Concurrency Paper]

  #link("https://github.com/microsoft/verona-rt/blob/main/docs/internal/concurrency/modelimpl/When.cs")[Basic C\# implementation]

  #link("https://github.com/microsoft/verona-rt/blob/main/src/rt/sched/behaviourcore.h")[Core C++ implementation]

  #link("https://www.youtube.com/watch?v=iX8TJWonbGU")[Presentation video]

  #link("https://github.com/microsoft/verona-artifacts/tree/main/WhenConcurrencyStrikes")[Supplementary]

  #link("https://github.com/kaist-cp/cs431")[KAIST CS431: Concurrent Programming]
]

#let transfer_example(func, qualifier) = box[$
  #func"(" qualifier("A") xarrow(sym: -->, "transfer 100") qualifier("B") ")      "
  #func"(" qualifier("B") xarrow(sym: -->, "transfer 100") qualifier("C") ")" \
  #func"(" qualifier("C") xarrow(sym: -->, "transfer 100") qualifier("D") ")      "
  #func"(" qualifier("D") xarrow(sym: -->, "transfer 100") qualifier("A") ")" \
$]

// Page 3
#polylux-slide[
  == Concurrency

  #side-by-side[
    Parallelism
    - Thread
    - Task
    - Coroutines
    - Async
    - ...
  ][
    Coordination
    - Promises
    - Locks
    - Condition variables
    - Transactions
    - ...
  ]
  #uncover(2)[#transfer_example("spawn", (x) => x)]
]

// Page 4
#polylux-slide[
  == Goal

  - #alternatives-match((
    "1": [Isolation],
    "2-3": [#text(fill: teal)[Isolation $->$ exclusive access (Mutex)]],
    "4-7": [Isolation $->$ exclusive access (Mutex)],
  ))
  - #alternatives-match((
    "1-3": [Parallelism],
    "4": [#text(fill: teal)[Parallelism]],
    "5-7": [Parallelism],
  ))
  - #alternatives-match((
    "1-4": [Deadlock Freedom],
    "5": [#text(fill: teal)[Deadlock Freedom $->$ Deadlock avoidance (Sort)]],
    "6-7": [Deadlock Freedom $->$ Deadlock avoidance (Sort)],
  ))
  - #alternatives-match((
    "1-5": [Ordering],
    "6": [#text(fill: teal)[Ordering $->$ DAG (Dependency Graph)]],
    "7": [Ordering $->$ DAG (Dependency Graph)],
  ))

  #alternatives-match((
    "1": [#transfer_example("spawn", (x) => x)],
    "2, 5-7": [#transfer_example("spawn", (x) => box[&mut #x])],
    "3": [$
      "spawn(" "&mut A" xarrow(sym: -->, "transfer 100") "&mut B)      "
      "spawn(" "&mut B" xarrow(sym: -->, "transfer 100") "&mut C)"
    $],
    "4": [
      #align(left, block($ "spawn(" "&mut A" xarrow(sym: -->, "transfer 100") "&mut B)" $))
      #align(left, block($ "spawn(" "&mut C" xarrow(sym: -->, "transfer 100") "&mut D)" $))
    ],
  ))
]

// Page 5
#polylux-slide[
  == BoC in nutshell

  - #alternatives-cases(("1,   3, 4, 5", "2"), case => [
    #set text(fill: teal) if case == 1
    Cown: protects a piece of separated data $->$ Mutex
  ])
  - #alternatives-cases(("1, 2,   4, 5", "3"), case => [
    #set text(fill: teal) if case == 1
    Behaviour: unit of concurrent execution $->$ Thread
  ])
  - #alternatives-cases(("1, 2, 3,   5", "4"), case => [
    #set text(fill: teal) if case == 1
    When: spawns a behaviour with a set of required cowns $->$ Spawn
  ])

  $
    "when(Cown<A>, Cown<B>; &mut A" xarrow(sym: -->, "transfer 100") "&mut B)" \
    "when(Cown<B>, Cown<C>; &mut B" xarrow(sym: -->, "transfer 100") "&mut C)" \
    "when(Cown<C>, Cown<D>; &mut C" xarrow(sym: -->, "transfer 100") "&mut D)" \
    "when(Cown<D>, Cown<A>; &mut D" xarrow(sym: -->, "transfer 100") "&mut A)" \
  $
]

// Page 6
#polylux-slide[
  == Abstraction

  Example:

  #sourcecode[```cpp
    when (c1)     { /* b1 */ }
    when (c3)     { /* b2 */ }
    when (c1, c2) { /* b3 */ }
    when (c1)     { /* b4 */ }
    when (c2, c3) { /* b5 */ }
    when (c3)     { /* b6 */ }
  ```]
]

// Page 7
#polylux-slide[
  == Implementation with lock

  - Additional count
  - Scheduled flag
]

// Page 8
#polylux-slide[
  == Implementation without lock

  - Behaviour, Request, and Cown all on heap
  - Pin semantics
]

#polylux-slide[
  #set align(horizon + center)
  #set text(size: 30pt, font: "Avenir Next LT Pro")
  = Thanks for watching!
]
