require_relative "commit"
require_relative "version"

module Vector
  class Release
    class << self
      def all!(dir)
        release_meta_paths = Dir.glob("#{dir}/*.cue").to_a

        release_meta_paths.
          collect do |release_meta_path|
            release_json = `cue export #{release_meta_path}/../../urls.cue  #{release_meta_path}`
            release_hash = JSON.parse(release_json)
            name = release_hash.fetch("releases").keys.first
            hash = release_hash.fetch("releases").values.first
            new(hash.merge({"name" => name}))
          end.
          sort_by(&:version)
      end
    end

    attr_reader :codename,
      :commits,
      :date,
      :name,
      :version,
      :whats_next

    def initialize(hash)
      @codename = hash.fetch("codename", "")
      @commits = hash.fetch("commits").collect { |commit_hash| Commit.new(commit_hash) }
      @date = hash.fetch("date")
      @name = hash.fetch("name")
      @version = Util::Version.new(@name)
      @whats_next = hash.fetch("whats_next")
    end
  end
end
