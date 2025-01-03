# BoC talk

parallelism and coordination

Decoupling parallelism and coordination

parallelism: thread: kernel thread user-level threads, coroutines, tasks, fork/spawn.
simple coordination: join, promises

coordination primitive: locks, transactions, condition variables.

```
A -> B
B -> C
C -> D
D -> A

A, B, C, D = Mutex<A>, Mutex<B>, Mutex<C>, Mutex<D>
A -> B
B -> C
C -> D
D -> A
```

new concurrency paradigm: Behaviour-Oriented Concurrency (BoC).

parallelism:

coordination: data-race free, deadlock free.

==

cown: concurrent owner, isolated resource

Behaviours are asynchronous units of work that explicitly state their required set of resources.

when: construct behavior and spawn this an asynchronous unit of compute

A BoC program is a collection of behaviours that each acquires zero or more resources, performs computation on them, which typically involves reading and updating them, and spawning new behaviours, before releasing the resources. 

When running, a behaviour has no other state than the resources it acquired. A behaviour can be run when it has acquired all of its resources, which is guaranteed to be deadlock free.


Cowns a cown protects a piece of separated data, meaning it provides the only entry point to that data in the program. A cown is in one of two states: available, or acquired by a behaviour.

Behaviours are the unit of concurrent execution. They are spawned with a list of required cowns and a closure. We define happens before, as an ordering relation which strengthens the “spawned before” relation by also requiring that the sets of cowns required by the two behaviours have a non-empty intersection:

Behaviour-oriented concurrency (or BoC) is intended as the sole concurrency feature of an underlying programming language.
[Behaviour-Oriented Concurrency](https://dl.acm.org/doi/10.1145/3622852)

[C# implementation](https://github.com/microsoft/verona-rt/blob/main/docs/internal/concurrency/modelimpl/When.cs)