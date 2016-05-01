(ns neb.server
  (:require [cluster-connector.remote-function-invocation.core :as rfi]
            [cluster-connector.distributed-store.core :refer [delete-configure is-first-node?
                                                              join-cluster with-cc-store leave-cluster] :as ds]
            [cluster-connector.sharding.core :refer [register-as-master checkout-as-master]]
            [neb.schema :as s]
            [neb.durability.serv.core :as dserv]
            [neb.utils :refer :all]
            [neb.trunk-store :refer [init-trunks dispose-trunks start-defrag stop-defrag
                                     init-durability-client start-backup stop-backup]]
            [neb.base :refer [schemas-lock]]
            [cluster-connector.sharding.DHT :as dht]))

(def cluster-config-fields [:trunks-size])
(def cluster-confiugres (atom nil))
(def confiugres (atom nil))
(def server-loaded (atom false))

(defn shutdown []
  (let [{:keys [schema-file]} @confiugres]
    (when schema-file (s/save-schemas schema-file))))

(defn stop-server []
  (println "Shuting down...")
  (reset! server-loaded false)
  (try-all
    (rfi/stop-server)
    (stop-backup)
    (stop-defrag)
    (dispose-trunks)
    (leave-cluster)
    (shutdown)))

(defn interpret-volume [str-volume]
  (if (number? str-volume)
    str-volume
    (let [str-volume (clojure.string/lower-case (str str-volume))
          num-part (re-find #"\d+" str-volume)
          unit-part (first (re-find #"[a-zA-Z]+" str-volume))
          multiply (Math/pow
                     1024
                     (case unit-part
                       \k 1 \m 2 \g 3 \t 4 0))]
      (long (* (read-string num-part) multiply)))))

(defn get-cluster-configures []
  @cluster-confiugres)

(defn start-server [config]
  (let [{:keys [server-group server-name port zk meta]} config]
    (join-cluster
      (or server-group :neb)
      server-name
      port zk meta
      :connected-fn
      (fn []
        (when-not @server-loaded
          (let [is-first-node? (is-first-node?)
                cluster-configs (select-keys config cluster-config-fields)
                cluster-configs (if is-first-node?
                                  cluster-configs
                                  (or (rfi/condinated-siblings-invoke 'neb.core/get-cluster-configures)
                                      cluster-configs))
                {:keys [trunks-size]} cluster-configs
                {:keys [memory-size schema-file data-path durability auto-backsync replication
                        keep-imported-backup recover-backup-at-startup]} config
                trunks-size (interpret-volume trunks-size)
                memory-size (interpret-volume memory-size)
                schemas (if is-first-node?
                          (s/load-schemas-file schema-file)
                          (rfi/condinated-siblings-invoke-with-lock schemas-lock 'neb.schema/get-schemas))
                trunk-count (int (Math/floor (/ memory-size trunks-size)))]
            (println "Loading Store...")
            (reset! cluster-confiugres cluster-configs)
            (reset! confiugres config)
            (s/clear-schemas)
            (s/load-schemas schemas)
            (init-trunks trunk-count trunks-size (boolean durability))
            (when data-path (dserv/prepare-backup-server data-path keep-imported-backup))
            (start-defrag)
            (register-as-master (* 50 trunk-count))
            (when durability (init-durability-client (or replication 1)))
            (when recover-backup-at-startup (dserv/recover-backup))
            (when (and durability auto-backsync) (start-backup))
            (rfi/start-server port)
            (reset! server-loaded true))))
      :expired-fn
      (fn []
        (try
          (if (> (count @dht/cluster-server-list) 1)
            (stop-server)
            (println "Zookeeper disconnected but it is the only server. Will not shutdown."))
          (catch Exception ex
            (clojure.stacktrace/print-cause-trace ex)))))))

(defn clear-zk []
  (delete-configure :schemas)
  (delete-configure :neb))