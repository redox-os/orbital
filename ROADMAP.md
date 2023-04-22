# Analysis and Proposal for rework of Orbital
First pass, Monday March 13th, Andrew Mackenzie

## Long-term questions
Questions about what is the long-term plan.
That doesn't prevent using it for quite a while and making improvements and additions to it, but knowing
where we want to go eventually can help take decisions along the way.

### Window Manager
* Should we continue to invest in orbital as the main (only?) compositor and window manager and desktop of redox?
* How does Cosmic fit in? What needs to be done to port Cosmic?

### Wayland compatibility for apps
* Should redox-os offer a Wayland compatible solutions to apps?
   * Should we port actual Wayland?
   * Should we contribute to one of the Rust Wayland implementations
   * Should we build our own?

### Frameworks
* Agree on the UI framework we want to _use_ across redox-os GUI apps for a consistent look and feel and ease of development (due to consistency and similarity between projects)
* To increase the selection of great (hopefully rust!) apps on redox-os, allow apps using any of the most used GUI frameworks to run
  * What can we do to move the Slint port forward? Can we come up with a plan for who does what regarding Slint on Redox, so more people can contribute?
  * What needs to be done to support Iced? Can we find a way forward with Iced?
  * Can we add support for egui?

### Cross-platform support
* Should Orbital work on Linux? 
* Should orbclient work on linux/macos in order to facilitate development of cross-platform apps using it?
* Should orbutils run on those other OS too?

### GPU Support
What is our plan to do accelerated graphics and high-end CPU-rendered graphics?

## Process and org
* Reduce the workload and dependency on Jeremy.
* Have a number of named maintainers who can lead this area
* have a roadmap that describes the plan
* Maintainers review, organize, label, prioritize issues according to the roadmap

## Functionalities
* Compositor
* Window Manager
    * Addition of title bar and window controls to a window, Orbital doesn't seem to be a "reparenting" window manager,
      the apps I see (orb-term etc.) are written explicitly for it, using orbclient
* Desktop
    * Launcher app - should it be replaceable?
    * Background app - should it be replaceable
    * Login app - should it be replaceable

## Components
I have found the following components (in recipes) that are related to Orbital

* Orbital - the binary of "orbital" compositor/window manager, includes orbital-core as a sub-crate in a sub-folder
* liborbital - haven't looked closely at this yet
* orbclient - a crate used by apps to run on orbital
* orbutils - a workspace crate with "calculator" and "orbutils" members
    * calculator - looks like it has been ported to Slint
    * orbutils - a workspace member crate with multiple binaries.
* orbterm - a GUI terminal for redox
* orbdata - haven't really looked at that yet.
* orbimage
* orbfont
* orbtk - deprecated?

# Recipes
These recipes (orbutils-launcher, orbutils-orblogin, orbutils-background) are for producing minimized images of Redox OS for low resource computers, where a desktop is available but not all applications included.

## Proposed Roadmap
* Decide what is the GUI toolkit/framework to be used going forward
  * Current efforts target Slint. Continue with that?
  * Find all references/usaged of orbtk, remove them with rewrites and then delete orbtk
* orbital
  * orbital-core is now a module of orbital and doesn't need to be a workspace member
  * Move launcher, background and orblogin from orbutils into orbital. They are not really optional utils and they (or some replacement) is needed by orbital. 
    * They can all still be separate binaries, modular and replaceable by other binaries
    * This would cleanup orbutils and make them real optional utils, and all of them (the whole crate) could build, test and run across redox, linux and macos - which is not possible now.
    * orbital project should build on redox, linux and macos, but redox is the only target os.
* keep orbutils as a separate project
    * Finish GUI port and cleanup. With each util a workspace member (shares dependencies in target and
      faster compiles) and we can remove the two-tiers in this crate at the moment.
    * Move "core" apps to orbital as above 
    * Move orbterm into orbutils as another optional util. Same toolkit, build etc. There are no dependents on the crate in crates.io (I checked) and dependencies within redox-os are on the built binary
    * orbutils should build on redox, linux and macos and be able to target the host os (they build, test and run on redox, linux, macos)
* Examples
  * ~~Examples are not compiled in CI. If what they show is covered in orbutils, consider deleting them and just 
referring people to orbutils~~ DONE
  * ~~It looks like the simple example in Orbital, is a duplication of an example in orbclient. If so, remove it and 
add a reference in the README.md to the other repo and it's examples~~ DONE
* Update all components/crates to the latest edition (2021)
  * ~~I see fields named "async" that is a reserved keyword and will need changing~~ DONE
* Add doc comments and doc tests to API methods for use by application developers
  * Deploy "cargo doc" generated docs somewhere
* Improve testing, stability and ease contributions
  * Remove any old travis CI files
  * Improve test coverage, have the tests run in CI and don't merge if not green
    * More extensive test coverage makes contributions easier and more reliable for all, but especially for new 
developers
  * All crates have CI/CD added to them to make sure they compile at least
      * Remove all obsolete references to travis CI?
      * orbutils has no gitlab CI running
      * TBD (see above) on which OSes
      * Modify repo settings to now allow merging red MRs?
* Would we consider bundling other utils in their place, if we find good rust-based alternatives?
* Ease adoption of redox via more feature parity of orbital
  * Make it easier for users to work with redox, coming from macos and linux (windows?) by implementing more desktop
features
* Provide a compelling reason to use redox
  * Add advanced features to orbital to help make using redox a standout experience
* Let users adapt redox to them, not force them to adapt to redox
  * More configurability, less hard coded (config files, settings apps, themes)
