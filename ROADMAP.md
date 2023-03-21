# Analysis and Proposal for rework of Orbital
First pass, Monday March 13th, Andrew Mackenzie

## Long-term questions
Here I raise a few questions about what is the long-term plan (or let's define one if it doesn't already exist).
That doesn't prevent using it for quite a while and making improvements and additions to it, but knowing
where we want to go eventually can help take decisions along the way.

* Should we continue to invest in orbital as the main (only?) compositor and window manager and desktop of redox?

## Functionalities
* Compositor
* Window Manager
    * Addition of title bar and window controls to a window, Orbital doesn't seem to be a "reparenting" window manager,
      the apps I see (orb-term etc.) are written explicitly for it, using orbclient
* Desktop
    * Launcher app - should it be replaceable?
    * Background app - should it be replaceable

## Components
I have found the following components (in recipes) that are related to Orbital

* Orbital - the binary of "orbital" compositor/window manager, includes orbital-core as a sub-crate in a sub-folder
* liborbital - haven't looked closely at this yet
* orbclient - a crate used by apps to run on orbital
* orbutils - a workspace crate with "calculator" and "orbutils" members
    * calculator - looks like it has been ported to Slint
    * orbutils - a workspace member crate with multiple binaries.

Dependency on orbtk and pending port to Slint?
There seems to be attempts to be cross-platform, but due to dependencies on redox::syscall they don't compile on Linux
* background
* character_map
* editor
* file_manager
* launcher
* orblogin
* viewer
* calendar
* orbutils-launcher
* orbutils-orblogin
* orbutils-background
* orbdata - haven't really looked at that yet.
* orbterm - a terminal that runs on redox and linux/macOS
* orbimage
* orbfont
* orbtk

# Slint rewrite
It looks like a slint rewrite of some orbutils started. I see that calculator has been ported, but no others.

Is SLINT the future direction of GUI toolkit for redox and orbutils

I still see references to orbtk which I understand is deprecated.
Continue work to move off orbtk, until the point we can remove it entirely?
* what is the replacement? Slint, Iced? Egui?

## Questions about that
* Why are there two versions of
    * orblogin and orbutils / orblogin
    * orblauncher and orbutils / launcher
    * orbackground and orbutils / orb background
* The builds on all the orbutils-* variants fail for me
  * Are they expected to build? Or are they deprecated? Could they be deleted and left to gitlab memory?

Can we remove these duplicates or merge them under orbutils?
* Can we make orbterm a part of orbutils, as "just one more" orbutil app?
* Many components attempt to support redox and other OS (via SDL)
    * Fo we want to continue that as a goal (which OS?)
    * Some (e.g. orbterm) compiles and runs just fine on Linux
    * Some of them no longer compile for Linux due to direct inclusion of syscall and no conditional code for "redox"
      target (like orbterm does)

* Is travis still used anywhere?

## Proposed Roadmap
* Decide what OS we want to actively support "utils" running on
    * If we think any of them have potential to be "good" apps and get use across platforms and attract contributions
      then it would make sense to support them. But if not, then just extra effort. Supporting them can make it easier to
      develop and improve them (until redox is the main work OS for contributors) as you can dev on linux/macOS.
* Decide what is the GUI toolkit/framework to be used going forward
* Review code organization to make development easier and remove confusion and duplication
  * orbital is made a workspace project
  * orbital-core is made a workspace member
      * If it is included by crates outside the orbital workspace, then maybe make it a lib.
  * orbutils can be kept as a separate workspace project, or absorbed as part of orbital workspace
      * Once GUI port and cleanup is done, each util can just be a workspace member (shares dependencies in target and
        faster compiles) and we can remove the two-tiers in this crate at the moment.
      * All "utils" are combined into one "orbutils" package, and they are all ported to use the same UI toolkit
        (slint, iced, egui, whatever). i.e. orbterm is moved into utils.
      * Such utils (i.e. other apps) should be able to be written by anyone. Any dependencies they _require_ on orbital should be exposed
        public API, via a lib of orbital. They depend on "orbital" (but just the lib part).
      * orbclient should be part of orbital and the API for client apps, and exposed as a lib.
          * That would allow some internal re-org between orbclient and orbital-core (e.g. "core" structs such as Color
            are IMHO part of orbital-core). Backwards compatibility for any app _outside_ combined orbital and orbutils
            can be taken care of by re-exports.
  * It looks like the simple example in Orbital, is a duplication of an example in orbclient
* Examples in orbital and some other places are not compiled in CI. If what they show is covered in orbutils, 
consider deleting them and just referring people to orbutils
* Update all components/crates to the latest edition (2021)
  * I see fields named "async" that is a reserved keyword and will need changing
* Add doc comments and doc tests to API methods for use by application developers
  * Deploy "cargo doc" generated docs somewhere
* Improve testing, stability and ease contributions
  * Improve test coverage, have the tests run in CI and don't merge if not green
    * More extensive test coverage makes contributions easier and more reliable for all, but especially for new developers
  * All crates have CI/CD added to them to make sure they compile at least
      * Remove all obsolete references to travis CI?
      * TBD (see above) on which OSes
* Would we consider bundling other utils in their place, if we find good rust-based alternatives?
* Ease adoption of redox via more feature parity of orbital
  * Make it easier for users to work with redox, coming from macos and linux (windows?) by implementing more desktop
features
* Provide a compelling reason to use redox
  * Add advanced features to orbital to help make using redox a standout experience
* Let users adapt redox to them, not force them to adapt to redox
  * More configurability, less hard coded (config files, settings apps, themes)